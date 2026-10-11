# FL Studio behaviour notes (levels, pan, clips, swing)

**Clean-room statement.** Everything here comes from Image-Line's public FL Studio
online manual (pages cited per point), paraphrased, plus behaviour the project owner
observes in their own FL Studio install (recorded in `docs/parity/owner-checks.md`).
Nothing was decompiled, disassembled or downloaded from FL Studio. Quotes are short
phrases only.

Manual root: https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/

Abbreviations for the cited pages:

- [CR] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/channelrack.htm
- [CSM] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/chansettings_misc.htm
- [LVL] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/mixer_levelsandmixing.htm
- [MIX] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/mixer.htm
- [DB] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/mixer_dB.htm
- [PS] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/songsettings_settings.htm
- [PL] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/playlist.htm
- [PLP] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/playlist_patterns.htm
- [TRK] https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/mixer_trackprops.htm

## 1. Channel volume knob

What the manual says:

- The channel volume knob runs 0%–100% [CSM]. (A separate Sampler "Levels
  adjustment" volume goes to 200%, i.e. up to twice as loud [CSM]; that is a
  different control.)
- Samples loaded into a Channel Sampler come in at the default channel volume of
  78%, which the manual equates to -5.2 dB of the original level. Setting the knob
  to 100% restores the original level [CR].
- The mixing page describes the same default as "55% volume, about -5.2 dB" [LVL].
  So 78% is the knob position and 0.55 is the linear amplitude: the knob is **not**
  linear gain.
- In general, 0 dB means a level of 1 (100%), and -6 dB is half amplitude [DB].

What it settles: 100% = 0 dB (gain 1.0); 78% = -5.2 dB (gain about 0.55). It does
**not** give the curve between those points or below 78%.

**Patternhaus decision (implemented in T7, `ph-engine` `plan::channel_gain`):** channel
volume is no longer linear gain. The stored knob value `v` stays 0..1 with default 0.78
(matches FL), and maps through a power curve fitted exactly to the two documented anchors:

    gain = v ^ 2.409      (0.78 -> -5.20 dB; 1.0 -> 0 dB; 0 -> silence)

The shape between the anchors is provisional until OC-1..OC-3 come back (prediction at
50%: -14.5 dB). Before T7 the linear mapping played the default 0.78 at -2.2 dB, 3 dB
louder than FL. Test: `channel_strip_follows_fl_volume_and_circular_pan`.

## 2. Channel pan knob

What the manual says:

- The channel pan knob is sent to the plugin, so its effect depends on the plugin;
  the manual recommends mixer pan for plugins that ignore it [CR].
- Sampler channels and Audio Clips follow the project's default **Circular** panning
  law [LVL]. The default Sampler state can be 8.2 dB below the recorded level
  [LVL], i.e. -5.2 dB (volume knob) plus -3 dB (centre pan).
- Panning law is a project setting with two choices, Circular and Triangular [PS]:
  - Circular: level drops 3 dB at centre pan; extremes are not reduced [PS].
  - Triangular: no compensation; loudness rises toward centre; overall about 3 dB
    lower than circular's extremes across the range [PS].
- The mixer page states Circular is the default [MIX].

What it settles: default law is Circular, -3 dB at centre, 0 dB at the extremes, and
a Triangular alternative exists. It does not give the exact curve shape, nor what
the far channel does at hard pan.

**Patternhaus decision (implemented in T7, `plan::channel_gain`):** channels use the
Circular law as a constant-power (sin/cos) law, which matches both settled anchors:

    p in [-1, +1];  theta = (p + 1) * pi / 4
    gain_L = cos(theta);  gain_R = sin(theta)     (centre: 0.7071 each = -3.01 dB)

Together with the volume curve, the default channel (78%, centre) plays at -8.2 dB, the
figure the manual gives for the default Sampler state [LVL]. The far side is silent at
hard pan; OC-4 confirms that. The Triangular option (a project setting `pan_law`) needs a
model field and is moved to M2; its shape is OC-5.

## 3. Mixer track fader

What the manual says:

- The level fader is the track's volume and is applied post-effects [MIX].
- On the meter scale, 0 dB ("almost top") is 100%, a level of 1 [DB]; the hint bar
  shows the value as faders move [DB].
- Insert tracks practically cannot clip (32-bit float); only Master or a track sent
  to an ASIO output can [MIX], [LVL].
- No page found gives the fader's maximum dB, its default position, its curve, or
  the Master default [MIX], [TRK], [LVL].

**Patternhaus decision:** not settled by the manual. Keep the current model (insert
linear gain 0..2, default 0.8; Master default 1.0) unchanged until OC-6..OC-8 report
the default dB, the top-of-travel dB and a mid-travel point. Then define the fader
as position `f` in 0..1 with `dB = curve(f)` fitted to those readings.

## 4. Mixer track pan

What the manual says:

- The mixer uses the project panning law, Circular by default [MIX], [PS].
- Mixer pan is not a balance control: it mixes one side of the stereo track into the
  other, so 100% pan gives the mono sum of L and R on one side [MIX].

**Patternhaus decision:** partly settled. Mixer pan should become "merge and pan"
(at full right `out_R = k * (L + R)`, `out_L = 0`), but the scale `k` and the curve in
between are not in the manual. Changing it on a guess would move every mix twice, so
T7 keeps the balance law for inserts and records the difference in the parity tracker
until OC-9 reports the numbers.

## 5. Playlist pattern clip longer than its pattern

What the manual says:

- Resized or sliced clips are a "window over the original data"; data still exists
  past the visible clip edges [PL].
- Resizing is Shift+drag on a clip edge in Draw/Paint mode; resizing from the left
  needs the Playlist menu option "Allow resizing from the left" [PL].
- Repeats are made by Paint-mode dragging or Duplicate (Ctrl+B) [PL]; Make unique
  clones a clip so edits do not affect other instances [PL].
- Pattern clips are arranged like other clips [PLP]. No page states what plays when
  a pattern clip extends past the end of its pattern.

**Patternhaus decision:** not settled by the manual. Keep "silence after the pattern
ends" until OC-10 says otherwise.

## 6. Swing

What the manual says:

- Global Swing is a 0–100% multiplier applied to all channels [CR].
- Channel Swing (per channel) is relative to global; 0% = no swing, 100% = the full
  global amount; menu presets 0/25/50/75/100% or custom [CR].
- Swing is active only if both global and channel swing are above 0% [CR].
- Swing changes the length of odd vs even steps: it holds the odd 16ths
  (1, 3, 5, ... 15) and delays the even ones [CR].
- The Channel Settings Swing Mix knob is a multiplier for the Channel Rack swing [CSM].
- Separate Channel Settings "Shift" delays note starts by up to one step (a 16th)
  [CSM].

**Patternhaus decision:** settled except for the maximum offset. Patternhaus already
delays only the even 16ths (odd 0-based steps) and keeps the odd ones on the grid, as
the manual describes. Its global swing `s` (0..1) delays them by `s` × half a step (a
dotted-16th feel at 100%). FL's maximum is not documented, so OC-11/OC-12 measure it.
Per-channel swing (relative to global) is an M2 item: M1 has no channel swing field.
