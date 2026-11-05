# DecentSampler preset (`.dspreset`): format specification

**Status:** M3/T1, first edition (2026-11-05). Written for the DecentSampler importer
(M3/T4), which is implemented from this document alone.

**Clean-room statement.** DecentSampler presets are XML files documented publicly by their
author in the *DecentSampler Developer Guide*. Facts here are restated from that guide in
our own words, with page citations. No player or converter code was read.

**Evidence tags:** **[S]** per spec (the Developer Guide, page given); **[D]** documented
elsewhere, not seen; **[I]** inferred; **[C]** confirmed on files (none yet: no corpus
examined); **[PH]** a Patternhaus decision where the guide leaves a choice.

**Cited sources** (Developer Guide, "stable" edition read 2026-11-05; the same pages exist
per version, e.g. `/en/1.15.4/`):

- [DS-INTRO] https://decentsampler-developers-guide.readthedocs.io/en/stable/introduction.html
- [DS-GROUPS] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-groups-element.html
- [DS-FX] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-effects-element.html
- [DS-UI] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-ui-element.html
- [DS-MIDI] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-midi-element.html
- [DS-MOD] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-modulators-element.html
- [DS-TAGS] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-tags-element.html
- [DS-BUS] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-buses-element.html
- [DS-SEQ] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-noteSequences-element.html
- [DS-BIND] https://decentsampler-developers-guide.readthedocs.io/en/stable/appendix-b-the-binding-element.html
- [DS-KS] https://decentsampler-developers-guide.readthedocs.io/en/stable/topic-keyswitches.html
- [DS-LIB] https://decentsampler-developers-guide.readthedocs.io/en/stable/the-dslibraryinfo-file.html

The guide is versioned with the player (1.x); features carry a minimum player version,
noted where the guide gives one.

---

## 1. File

- XML 1.0, declared UTF-8 [S, DS-INTRO]. Parse with a conforming XML parser; reject files
  that are not well-formed, but report rather than reject unknown elements and attributes
  [PH].
- **Element and attribute names are case-sensitive** [I]; effect type names must be
  lowercase, and an unknown type is silently ignored by the player [S, DS-BUS]. The
  importer reports unknown names [PH].
- Numbers use `.` as the decimal point [S, DS-INTRO].
- Booleans: `true`/`false`; accept `1`/`0` as well [I, the guide shows `1/0` for one
  modulator parameter].
- **Paths** (`path`, `irFile`, `bgImage`, `image path`, `wavetableFile` …) are relative to
  the folder of the `.dspreset` file [S, DS-LIB]. Accept `/` and `\`; try exact case, then
  case-insensitive per component [PH].
- Containers [I, not on the pages read]: a `.dsbundle` is a folder (macOS bundle-style)
  holding presets and samples; a `.dslibrary` is a zip archive of such a folder. M3 reads
  `.dspreset` files directly and `.dsbundle` folders; `.dslibrary` zips are opened read-only
  in place if `ph-formats` has zip support, else refused with a message [PH].
- `DSLibraryInfo.xml` (optional, library root): `<DecentSamplerLibraryInfo name productId
  version coverArt>` with a `<presetMenu>` of nested `<menu name>` and `<preset file>`.
  Keep as library metadata only [S, DS-LIB].

## 2. Document structure [S, DS-INTRO and the element pages]

```
<DecentSampler minVersion=".." limiterStyle="default|brickwall">
  <ui ...> ... </ui>                     optional
  <groups ...> <group ...> <sample .../> <oscillator .../> <effects/> </group> ... </groups>   required
  <effects> <effect .../> ... </effects> optional, instrument-level chain
  <midi> <cc/> <note/> <velocity/> </midi>  optional
  <modulators> <lfo/> <envelope/> ... </modulators> optional
  <tags> <tag/> ... </tags>              optional
  <buses> <bus> <effects/> </bus> ... </buses> optional
  <noteSequences> <sequence> <note/> </sequence> </noteSequences> optional
  <arpeggiator .../>                     optional
</DecentSampler>
```

- Root attributes: `minVersion` (minimum player version string), `limiterStyle`
  (`default` = soft compression above −10 dBFS with makeup, `brickwall` = ceiling only;
  default `default`) [S, DS-INTRO]. Keep both as data; the limiter is approximated
  (skipped, reported) [PH].
- Child order is free [I]. Only `<groups>` is required [S].

---

## 3. Groups and samples [S, DS-GROUPS]

### 3.1 Inheritance

- Most `<sample>` attributes may also be set on `<group>` and `<groups>`; the **most
  specific level wins** [S].
- **Exception: volume does not inherit; the three levels multiply**
  (`groups.volume × group.volume × sample.volume`) [S].
- **Pan inherits** (the most specific `pan` wins); `globalPan` (groups) and `groupPan`
  (group) are separate offsets **added** to the sample's pan; the sum is clamped to ±100 [S].
- Tuning: `globalTuning` + `groupTuning` + `tuning` add [S for each being a shift; I for
  the sum].
- Groups are indexed from 0 in document order; bindings address groups by that index
  [S, DS-BIND].

### 3.2 Volume values [S]

`volume`, `globalVolume`, `groupVolume`: either **linear** (`0.0`–…, `1.0` = unity) or
**dB** with a `dB` suffix (`"-3dB"`, `"3dB"`). Parse the suffix case-insensitively [PH].
`globalVolume` is an alias of `volume` on `<groups>` and wins if both are set;
`groupVolume` likewise on `<group>` [S].

### 3.3 `<groups>` attributes [S]

| Attribute | Type | Default | Meaning | M3 |
|---|---|---|---|---|
| `volume` / `globalVolume` | linear or dB | 1.0 | instrument gain | must |
| `globalPan` | −100–100 | 0 | pan offset | must |
| `globalTuning` | semitones (float) | 0 | | must |
| `tuningIgnoresKeyTrack` | bool | false | tuning applies fully even with reduced key tracking (1.36.1) | must |
| `glideTime` | s | 0 | portamento time | must |
| `glideMode` | `always`, `legato`, `off` | `legato` | | must |
| all inheritable sample/group attributes | | | defaults for the levels below | must |

### 3.4 `<group>` attributes [S]

| Attribute | Type | Default | Meaning | M3 |
|---|---|---|---|---|
| `name` | string | — | group name [I: used widely, not in the table read] | must (data) |
| `enabled` | bool | true | a disabled group does not play; keyswitches toggle this (5.3) | must |
| `volume` / `groupVolume` | linear or dB | 1.0 | | must |
| `groupPan` | −100–100 | 0 | offset | must |
| `groupTuning` | semitones | 0 | | must |
| `ampVelTrack` | 0–1 | not stated; **[PH] 1.0** | 1.0: velocity 127 → gain 1, 63 → ≈ 0.5 (linear in velocity) | must |
| `pitchKeyTrack` | 0–1 | 1 | 0 = fixed pitch | must |
| `ampEnvEnabled` | bool | true | false = one-shot: plays to the end of the file regardless of note-off | must |
| `tags` | comma list | — | | must |
| `silencedByTags`, `silencingMode`, `silencingDecay` | | | as sample (3.5) | must |
| `seqMode`, `seqLength`, `seqPosition` | | | as sample | must |
| `trigger`, `releaseTriggerDecay`, `loCCN`/`hiCCN`, `onLoCCN`/`onHiCCN` | | | as sample | must |
| loop, envelope and curve attributes | | | as sample | must |
| `outputNTarget`, `outputNVolume` | | | as sample (3.5) | must |
| `polyphony` | int | unlimited | voices for the group's oscillators (and samples [I]) | must |
| slice and retrigger attributes | | | as sample | skip |
| `<effects>` child | | | per-group effect chain, instantiated per voice [S, DS-FX] | must (approximate per group) |

### 3.5 `<sample>` attributes [S]

**Mapping and playback**

| Attribute | Type | Default | Meaning | M3 |
|---|---|---|---|---|
| `path` | path | required | WAV, AIFF or FLAC | must |
| `rootNote` | key | required | unshifted key | must |
| `loNote` / `hiNote` | key | 0 / 127 | key range, inclusive | must |
| `loVel` / `hiVel` | 0–127 | 0 / 127 | velocity range, inclusive | must |
| `start` / `end` | frames | 0 / last frame | play range; `end` is the last frame played (inclusive) [S: default "file length − 1"] | must |
| `tuning` | semitones (float) | 0 | | must |
| `volume` | linear or dB | 1.0 | multiplies with group and groups | must |
| `pan` | −100–100 | 0 | | must |
| `pitchKeyTrack` | 0–1 | 1 | | must |
| `delay` | float | 0 | start delay | must |
| `delayUnit` | `seconds`, `beats`, `samples` | `seconds` | | must (beats at project tempo) |
| `playbackMode` | `memory`, `disk_streaming`, `auto` | `auto` | loader hint | ignore (we always stream) [PH] |
| `sampleMode` | `normal`, `slice_stretch` | `normal` | tempo-synced slices (1.37.0) | skip `slice_stretch` |
| `tags` | comma list | — | for bindings and silencing | must |

**Triggers and conditions**

| Attribute | Type | Default | Meaning | M3 |
|---|---|---|---|---|
| `trigger` | `attack`, `release`, `first`, `legato`, `continuous` | `attack` | `release` plays at note-off; `first` only with no other note sounding; `legato` only with another note sounding; `continuous` always plays (both cases) [S] | must |
| `releaseTriggerDecay` | dB/s (`"3dB"`) or linear 0–1 | 0 | release-sample level falls with the time the note was held; a positive dB value is taken as a decrease [S]. Linear form: **[PH]** gain factor applied per second | must (dB form), approximate (linear form) |
| `loCCN` / `hiCCN` (e.g. `loCC64`) | 0–127 | −1 (off) | plays only if CC N's last value is in range; both must be set | must |
| `onLoCCN` / `onHiCCN` | 0–127 | −1 (off) | the CC **entering** the range triggers the sample (pedal noises) | must |
| `previousNotes` | comma list of keys | — | plays only if the previous note is in the list | must |
| `legatoInterval` | signed semitones | — | plays only if the previous note was exactly this far away (previous = target − interval) | must |

**Round robin**

| Attribute | Values | Default | Meaning | M3 |
|---|---|---|---|---|
| `seqMode` | `always`, `round_robin`, `random`, `true_random` | `always` | `always` = no round robin; `round_robin` cycles by position; `random` picks randomly but never the same zone twice in a row (with more than two); `true_random` picks with no such check | must |
| `seqPosition` | int ≥ 1 | 1 | position in the cycle | must |
| `seqLength` | int | 0 = automatic | cycle length; 0 = the player works it out | must |

Round-robin rules for the engine [I] [PH]:
- The "set" is all samples that match the note (key, velocity, CC, trigger) and have the
  same `seqMode`. Automatic length = the highest `seqPosition` in the set.
- `round_robin`: one counter per set, advanced on each note-on that matches; play the
  samples whose `seqPosition` equals the counter. **[PH]:** counters are keyed by the group
  that holds the samples plus the matching key range, so separate keys of a multi-sampled
  instrument cycle independently.
- `random` / `true_random`: pick a position 1…length from the seeded generator; for
  `random`, redraw once if it equals the previous pick for that set.

**Loops**

| Attribute | Type | Default | Meaning | M3 |
|---|---|---|---|---|
| `loopStart` / `loopEnd` | frames | from the file's loop markers if any, else 0 / last frame | loop range, inclusive end [I] | must |
| `loopEnabled` | bool | not stated; **[PH]** true if the file has a loop marker or `loopStart`/`loopEnd` is set, else false | | must |
| `loopCrossfade` | frames | 0 = off | crossfade length | must |
| `loopCrossfadeMode` | `linear`, `equal_power` | `equal_power` | | must |
| `loopDuringRelease` | bool | true | false: leave the loop at note-off and play to the end (1.36.1) | must |

**Amplitude envelope (AHDSR with delay)**

| Attribute | Type | Default | M3 |
|---|---|---|---|
| `ampEnvEnabled` | bool | true | must |
| `ampEnvDelay` | s | 0 (silent wait; the note's timing is unchanged) | must |
| `attack` / `ampEnvAttack` | s | not stated; **[PH]** 0 | must |
| `ampEnvHold` | s | 0 | must |
| `decay` / `ampEnvDecay` | s | not stated; **[PH]** 0 | must |
| `sustain` / `ampEnvSustain` | 0–1 | not stated; **[PH]** 1 | must |
| `release` / `ampEnvRelease` | s | not stated; **[PH]** 0.1 | must |
| `attackCurve` | −100–100 | −100 (logarithmic) | approximate |
| `decayCurve` | −100–100 | 100 | approximate |
| `releaseCurve` | −100–100 | 100 | approximate |

The `ampEnv…` spelling wins when both are present [S]. Curve scale: −100 logarithmic,
0 linear, 100 exponential [S]. **[PH]:** map curves to Haus Sampler's curve parameter
linearly if T2 supports one; otherwise snap to linear/exponential and report.

**Voice silencing (choke)**

| Attribute | Values | Default | Meaning | M3 |
|---|---|---|---|---|
| `silencedByTags` | comma list of tags | — | this sample stops when a sample with one of these tags starts | must |
| `silencingMode` | `fast`, `normal` | `fast` | `fast` cuts at once, `normal` enters the release | must |
| `silencingDecay` | s | 0 | custom fade; overrides the mode when > 0 | must |

**Routing**

| Attribute | Values | Default | M3 |
|---|---|---|---|
| `output1Target` | `MAIN_OUTPUT`, `NO_OUTPUT`, `BUS_1`–`BUS_16`, `AUX_STEREO_OUTPUT_1`–`_16` | `MAIN_OUTPUT` | must (AUX → multi-out) |
| `output2Target`…`output8Target` | same | `NO_OUTPUT` | must |
| `output1Volume`…`output8Volume` | 0–1 | 1.0 | must |

**Retrigger** (`retriggerEnabled`, `retriggerInterval` default 4.0, `retriggerIntervalUnit`
`seconds|beats|samples` default `beats`) [S]: skip and report.

**Slice stretch** (`beats`, `originalTempo`, `slices`, `sliceGrid` default `1/16`,
`sliceLoopMode` `off|forward|back_and_forth` default `forward`, `sliceEnvelope` 0–100
default 100; 1.37.0) [S]: skip and report; keep as data.

### 3.6 `<oscillator>` (1.15.0) [S]

`waveform` = `sine` (default), `saw`, `square`, `triangle`, `noise`/`white_noise`,
`pluck1`, `wavetable`, `harmonic`, `formant`, `fm6op`, with type-specific attributes
(`damping`, `pluckType`, `wavetableFile`, `wavetableFrameSize` 2048, `wavetablePosition`,
`numPartials`, `harmonicPartialNLevel`, `fmAlgorithm`, `fmOpN…` …) and `polyphony`.
Oscillators respond to the group's range, envelope, volume, pan, tuning and glide [S].
M3 **[PH]:** skip all oscillators, keep them as data, and report. (A later milestone may
map `sine`/`saw`/`square`/`triangle`/`noise` to built-in synth voices.)

### 3.7 Pitch [I]

semitones = `(key − rootNote) × pitchKeyTrack + tuning + groupTuning + globalTuning`
(with `tuningIgnoresKeyTrack` the tunings are not scaled; without it the guide implies they
are scaled by key tracking for samples [I]); ratio = 2^(semitones/12) × file rate / engine
rate.

### 3.8 Velocity [S for ampVelTrack anchors; I for the curve]

gain = `1 − ampVelTrack × (1 − velocity/127)`. (At 1.0: 127 → 1, 63 → ≈ 0.5, matching
the guide's examples.)

---

## 4. Effects [S, DS-FX]

`<effects>` holds `<effect type="…">` elements in chain order. Placement:
- directly under `<DecentSampler>`: the instrument chain;
- inside a `<group>`: per-voice chain for that group (created per note) — the guide notes
  only filters, gain and chorus work there; delay and reverb tails are cut [S];
- inside a `<bus>` (5.5).

Every effect also takes `enabled` (default true) and `tags`.

| Type | Attributes (range, default) | M3 mapping [PH] |
|---|---|---|
| `lowpass` (also `lowpass_4pl`), `bandpass`, `highpass` | `frequency` 0–22000 Hz (22000), `resonance` 0.001–5 (0.7) | Haus filter, 2-pole; supported |
| `lowpass_1pl` | `frequency` (22000) | 1-pole; supported |
| `notch` | `frequency` 60–22000 (10000), `Q` 0.01–18 (0.7) | supported |
| `peak` | `frequency` (10000), `Q` (0.7), `gain` linear 0.0001–10 or dB −80–20 with `gainUnit="decibels"` (1.0) | supported |
| `low_shelf` / `high_shelf` (1.34.0) | `frequency` (200 / 5000), `q` (0.7), `gain`, `gainUnit` | supported |
| `gain` | `level` dB −100–24 (about −6) or linear 0–8 with `levelUnit="linear"` | supported |
| `compressor` | `threshold` −60–0 dB (−12), `ratio` 1–20 (4), `attack` 0.1–200 ms (5), `release` 5–2000 ms (100), `inputGain`, `outputGain` ±24 dB (0), `autoBypass` (false) | built-in compressor; approximated |
| `reverb` | `roomSize` 0–1 (0.7), `damping` 0–1 (0.3), `wetLevel` 0–1 (0) | built-in reverb; approximated |
| `delay` | `delayTimeFormat` `seconds`/`musical_time`, `delayTime` 0–20 s (0.7), `feedback` 0–1 (0.2), `stereoOffset` ±10 (0), `feedbackCutoff` 20–22000 Hz (22000 = off), `wetLevel` 0–1 (0.5) | built-in delay; approximated |
| `chorus` | `mix` 0–1 (0.5), `modDepth` 0–1 (0.2), `modRate` 0–10 Hz (0.2) | `ph.fx.chorus`; approximated |
| `phaser` | `mix`, `modDepth`, `modRate`, `centerFrequency` 0–22000 (400), `feedback` −1–1 (0.7) | `ph.fx.phaser`; approximated |
| `convolution` | `mix` 0–1 (0.5), `irFile` (required; WAV/AIFF) | `ph.fx.convolution` with the IR copied into the instrument; supported |
| `stereo_simulator` (1.17.0) | `algorithm` `lauridsen`/`schroeder`/`adt` (adt), `width`, `delayTime`, `modRate`, `modDepth` | `ph.fx.stereo`; approximated |
| `pitch_shift` | `pitchShift` ±24, `mix` | skip |
| `wave_folder` (1.7.2) | `drive` 1–100 (1), `threshold` 0–10 (0.25) | skip |
| `wave_shaper` (1.7.2) | `drive` 1–1000, `driveBoost`, `outputLevel`, `highQuality` | skip |
| `bit_crusher` | `bitDepth` 1–24 (24), `sampleRateReduction` 1–32 (1), `mix` (1.0) | `ph.fx.bitcrush`; approximated |
| `stutter` | `amount`, `mix` | skip |

Every effect element is kept as data in the native instrument even when skipped [PH].

---

## 5. Control data: UI, MIDI, modulators, tags, buses

M3 does not run DecentSampler UIs (skins are M3b, see M3.md decision 1). The importer
**reads and keeps** `<ui>` as data, and **wires bindings** that target sound parameters to
native instrument controls where it can, so a knob in the generic panel still does what
the preset's knob did [PH].

### 5.1 Bindings [S, DS-BIND]

`<binding>` appears inside UI controls, `<midi>` children and modulators.

| Attribute | Values |
|---|---|
| `type` (required) | `amp`, `general`, `effect`, `control`, `note`, `note_binding`, `velocity_binding`, `button_state_binding`, `keyboard_color`, `modulator`, `note_sequence`, `arpeggiator` |
| `level` (required) | `ui`, `instrument`, `group`, `sample`, `oscillator`, `tag`, `midi` |
| `position` | 0-based index of the target; typed aliases `controlIndex`, `groupIndex`, `effectIndex`, `modulatorIndex` |
| `tags`, `groupTags`, `sampleTags`, `effectTags`, `modulatorTags`, `controlTags` | target by tag instead of index |
| `identifier` | target name (the tag when targeting by tag) |
| `parameter` (required) | parameter token (below) |
| `factor` | input multiplier before translation (1) |
| `translation` | `linear` (default), `table`, `fixed_value` |
| `translationOutputMin` / `translationOutputMax` | linear output range |
| `translationReversed` | bool (false) |
| `translationTable` | `in,out;in,out;…` (≥ 2 pairs; default `0,0;1,1`), piecewise linear |
| `translationValue` | value for `fixed_value` |
| `triggerOnLoad` | bool (true): apply the control's initial value at load |
| `modBehavior` (modulator bindings) | `add`, `modulate`, `multiply`, `set` (default `set`) [S, DS-MOD] |
| `enabled` | bool |

Parameter tokens M3 must map [S, DS-BIND]:
- **amp, instrument:** `AMP_VOLUME` (0–16 linear), `GLOBAL_TUNING` (±36), `PAN` (±100),
  `AMP_VEL_TRACK`, `ENV_ATTACK`/`ENV_DELAY`/`ENV_HOLD` (0–10 s), `ENV_DECAY`/`ENV_RELEASE`
  (0–25 s), `ENV_SUSTAIN` (0–1), `ENV_*_CURVE` (±100), `GLIDE_TIME`, `GLIDE_MODE`.
- **amp, group:** `ENABLED`, `AMP_ENV_ENABLED`, `AMP_VOLUME`, `GROUP_TUNING`, `PAN`,
  `AMP_VEL_TRACK`, the `ENV_*` set, `OUTPUT_N_VOLUME`, `OUTPUT_N_TARGET`, glide.
- **general:** `SAMPLE_START`, `SAMPLE_END`, `LOOP_START`, `LOOP_END` (frames),
  `LO_NOTE`, `HI_NOTE`, `ROOT_NOTE`, `LO_VEL`, `HI_VEL`, `SILENCING_DECAY`,
  `SILENCING_MODE`, `PITCH_KEY_TRACK`, `TAG_VOLUME`, `TAG_ENABLED`, `TAG_POLYPHONY`
  [S for the first eleven; I for the `TAG_*` names, which the tags page mentions only
  as `TAG_POLYPHONY`].
- **effect:** `FX_MIX`, `FX_FILTER_FREQUENCY`, `FX_FILTER_Q`, `FX_FILTER_GAIN`,
  `FX_FILTER_RESONANCE`, `FX_REVERB_WET_LEVEL`, `FX_REVERB_ROOM_SIZE`,
  `FX_REVERB_DAMPING`, `FX_MOD_DEPTH`, `FX_MOD_RATE`, `FX_CENTER_FREQUENCY`,
  `FX_FEEDBACK`, `FX_FEEDBACK_CUTOFF`, `FX_DELAY_TIME`, `FX_STEREO_OFFSET`,
  `FX_WET_LEVEL`, `LEVEL` (gain), `ENABLED`.
- Others (`control`, `keyboard_color`, `note_sequence`, `arpeggiator`, oscillator, slice,
  `FX_IR_FILE`, `FX_PITCH_SHIFT`, `FX_DRIVE*`, `FX_THRESHOLD`, `FX_OUTPUT_LEVEL`): keep as
  data, report as skipped.

### 5.2 `<ui>` [S, DS-UI]

`<ui width height bgColor [coverArt] [bgImage]>` with `<tab>` children and an optional
`<keyboard centerNote>` holding `<color loNote hiNote color>` ranges. Controls:
`labeled-knob` and `control` (`x y width height parameterName label style valueType
value minValue maxValue defaultValue …`, child bindings), `button` (`<state>` children
with bindings, `value` = state index), `menu` (`<option>` children with bindings, `value`
= 1-based option, 0 = none), `xyPad` (`<x>`, `<y>` children), `drawbars`. Display only:
`label`, `image`, `multiFrameImage`, `rectangle`, `line`, `oscilloscope`. Colours are
`AARRGGBB` hex with a leading `#` or `0x`-free form [S, appendix A, not read in full; I
for the exact parse].

M3 **[PH]:**
- Every `labeled-knob`/`control` with at least one mapped binding becomes a native
  instrument control: name = `label` (or `parameterName`), range = `minValue`…`maxValue`,
  value = `value`, and the bindings' translations become its mapping.
- `button` and `menu` become stepped controls with one setting per state/option.
- `<keyboard><color>` ranges become keyboard colour hints in the zone map.
- Everything else is kept verbatim (the whole `<ui>` subtree as a string) for M3b.

### 5.3 `<midi>` and keyswitches [S, DS-MIDI, DS-KS]

- `<cc number="N">` with bindings: CC N drives the bound parameters (input 0–127 scaled
  to 0–1 before translation [I]).
- `<note note="K" | "K1-K2" eventType="note_on|note_off|any|first_note_on|last_note_off"
  enabled swallowNotes>` with bindings; `eventType` default `any`; `swallowNotes` (default
  false) stops the note from also playing samples [S].
- `<velocity>` with bindings (and `modAmount`).
- **Keyswitches** are `<note>` listeners whose bindings set group `ENABLED` with
  `translation="fixed_value"` and `translationValue="true|false"` [S, DS-KS]. The initial
  articulation is whichever groups are `enabled` in the file [I].
- M3 **[PH]:** the importer recognises this pattern (a `<note>` whose bindings only set group
  `ENABLED` values) and turns each such note into a native **keyswitch** that selects the
  set of enabled groups; `swallowNotes="true"` marks the key as silent. Other `<note>`
  bindings are kept as data and reported.

### 5.4 `<modulators>` [S, DS-MOD]

| Element | Attributes (default) | M3 |
|---|---|---|
| `<lfo>` | `shape` `sine`/`square`/`saw` (bindings also list `triangle`), `frequency` (Hz), `frequencyFormat` `hz`/`musical_time`, `modAmount` 0–1 (1), `delayTime` s (0), `scope` `global`/`voice` (global) | must (to the modulation matrix) |
| `<envelope>` | `delay` (0), `attack`, `hold` (0), `decay`, `sustain`, `release`, curves (−100/100/100), `modAmount` (1), `scope` (global) | must |
| `<midiCC>` | `number` (required), `modAmount`, `channel` | must |
| `<midiVelocity>` | `modAmount`, `scope` | must |
| `<random>` | `mode` `note_on`/`periodic`, `frequency`, `trigger` `attack`/`none`, `seed`, `scope`; output −1…1 | must (`note_on`), approximate (`periodic`) |
| `<mpeTimbre>`, `<mpePressure>` | smoothing times | skip |

Each modulator holds `<binding>` children naming targets (5.1) and a `modBehavior`.
Modulation is applied on top of the stored value and never changes it [S]. Only some
targets are modulatable: most effect parameters, group and global volume, pan and tuning
[S]. `scope="global"` = one modulator for the instrument (free-running); `voice` = one per
note [S].

### 5.5 `<tags>` and `<buses>` [S, DS-TAGS, DS-BUS]

- `<tag name enabled=true volume=1.0 polyphony=-1>`: per-tag enable, gain and voice limit
  (−1 = unlimited, 1 = mono). Samples and groups join tags through their `tags`
  attribute. **[PH]:** a tag with a voice limit becomes a native voice-limit group; tag
  volume and enable become controls.
- `<buses>` holds up to 16 `<bus busVolume output1Target… output1Volume…>` with an
  `<effects>` chain. Samples reach a bus via `outputNTarget="BUS_n"`. **[PH]:** each used bus
  becomes an output group with its own effect chain (per-group effects in M3 scope).

### 5.6 `<noteSequences>` and `<arpeggiator>` [S, DS-SEQ]

`<sequence name length rate>` with `<note position velocity note length>` (beats). Played
through `note_sequence` bindings. M3: keep as data, skip, report. Same for
`<arpeggiator>`.

---

## 6. Version notes [S]

| Player version | Adds |
|---|---|
| 1.4.0 | keyswitches through `<midi><note>` (DS-KS covers 1.4.0 and later) |
| 1.7.2 | `wave_folder`, `wave_shaper` |
| 1.15.0 | `<oscillator>` |
| 1.17.0 | `stereo_simulator` |
| 1.34.0 | shelves, `gainUnit`, `feedbackCutoff`, `first_note_on`/`last_note_off`, modulator `ENABLED` |
| 1.36.1 | `tuningIgnoresKeyTrack`, `loopDuringRelease` |
| 1.37.0 | `slice_stretch` |
| 1.39.0 | `.dssnapshot` presets in the library menu |

The importer reads `minVersion` for the report only; it never refuses a file by version.

---

## 7. Unknowns

1. Defaults the guide leaves unstated: `ampVelTrack`, envelope times, `loopEnabled`.
   Our [PH] values are in the tables; a player listening test by the owner would settle them.
2. Round-robin counter scope (3.5).
3. Exact envelope curve formula for `attackCurve`/`decayCurve`/`releaseCurve`.
4. Whether tunings are scaled by `pitchKeyTrack` when `tuningIgnoresKeyTrack` is false.
5. `.dslibrary` packaging (zip assumed).
6. The colour format (appendix A not read in full).

---

## 8. Fixture plan (generated, never copied)

Fixtures are XML written by test code, with soundgen samples.

| Id | Content | Exercises |
|---|---|---|
| F-DS-01 basic | one group, three samples with `rootNote`/`loNote`/`hiNote`/`loVel`/`hiVel` | mapping |
| F-DS-02 inheritance | attributes on `<groups>`, `<group>`, `<sample>`; volumes in linear and dB at all three levels; `globalPan`+`groupPan`+`pan` | multiply vs. inherit rules |
| F-DS-03 round robin | `seqMode` each value, explicit and automatic `seqLength` | round robin |
| F-DS-04 triggers | `release` with `releaseTriggerDecay="3dB"`, `first`, `legato`, `continuous`, `previousNotes`, `legatoInterval` | triggers |
| F-DS-05 cc | `loCC1`/`hiCC1` layers; `onLoCC64`/`onHiCC64` pedal sample | CC conditions |
| F-DS-06 loops | file loop markers; explicit `loopStart`/`loopEnd`; `loopCrossfade` both modes; `loopDuringRelease=false`; `ampEnvEnabled=false` | loops, one-shot |
| F-DS-07 envelope | all AHDSR attrs incl. aliases both spelled; curves | envelope |
| F-DS-08 choke | hi-hat with `tags` and `silencedByTags`, each `silencingMode`, `silencingDecay` | silencing |
| F-DS-09 keyswitch | two groups toggled by `<midi><note>` `ENABLED` bindings, `swallowNotes`, keyboard colours | keyswitch recognition |
| F-DS-10 effects | instrument chain of each effect type, group-level lowpass, a bus with reverb | effects mapping, data kept |
| F-DS-11 ui | knobs bound to `AMP_VOLUME` (linear), `FX_FILTER_FREQUENCY` (table translation), a menu, a button, an unmappable control | control mapping |
| F-DS-12 modulators | voice LFO to group tuning, global LFO to filter, envelope, midiCC, random | modulation matrix |
| F-DS-13 misc | tags with polyphony, `outputNTarget` to AUX, oscillator (skipped), noteSequences (kept), unknown element, Windows-case path | report and data retention |
| F-DS-14 bundle | a `.dsbundle` folder with `DSLibraryInfo.xml` and two presets | containers |
