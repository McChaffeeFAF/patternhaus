# SFZ instrument file (`.sfz`): format specification

**Status:** M3/T1, first edition (2026-11-05). Written for the SFZ importer (M3/T3), which is
implemented from this document alone.

**Clean-room statement.** SFZ is an open, text-based format with a public reference. Every
fact here comes from the public opcode and header reference at sfzformat.com (cited per
section) and from general knowledge of how SFZ players behave, stated in our own words. No
player or converter source code was read for this document. Where players disagree, the
spec says so and picks one behaviour for Patternhaus, marked **[PH]** (our decision, not a
fact about the format).

**Evidence tags** used on every fact:

- **[S]** per spec: stated by the sfzformat.com reference;
- **[C]** confirmed on files: seen in files we generated or freely redistributable files;
- **[D]** documented, not seen: stated by a secondary public source (named), not in the reference;
- **[I]** inferred: common player behaviour or a reading the reference implies but does not state;
- **[PH]** Patternhaus decision where players or the reference leave a choice.

No corpus has been examined yet, so nothing is **[C]** in this edition. The fixture plan
(section 13) lists the files that turn the important **[I]** items into tested behaviour.

**Cited sources:**

- [SFZ-H] https://sfzformat.com/headers/ (header list, versions, hierarchy)
- [SFZ-OP] https://sfzformat.com/opcodes/ (opcode index with versions and categories)
- [SFZ-TRIG] https://sfzformat.com/opcodes/trigger/
- [SFZ-DEF] https://sfzformat.com/opcodes/define/
- [SFZ-INC] https://sfzformat.com/opcodes/include/
- [SFZ-SW] https://sfzformat.com/opcodes/sw_last/ and https://sfzformat.com/opcodes/sw_lolast/
- [SFZ-LFOW] https://sfzformat.com/opcodes/lfoN_wave/
- [SFZ-FIL] https://sfzformat.com/opcodes/fil_type/
- [SFZ-XF] https://sfzformat.com/opcodes/xf_cccurve/ and https://sfzformat.com/opcodes/xfin_loccN/
- [SFZ-LOOP] https://sfzformat.com/opcodes/loop_mode/
- [SFZ-CURVE] https://sfzformat.com/headers/curve/
- [SFZ-BASICS] https://sfzformat.com/tutorials/basics/ (syntax, note naming)
- Individual opcode pages follow the pattern `https://sfzformat.com/opcodes/<name>/`; a fact
  cited as [S] in a table row comes from that opcode's own page unless another source is named.

Conventions: "key" is a MIDI note number 0–127; "C4 = 60" (see 3.3). Times are seconds,
levels dB unless the table says otherwise. "Region" is a playable zone. Versions: **v1**
(SFZ 1.0), **v2** (SFZ 2.0), **ARIA** (extensions by Plogue's ARIA engine, the de-facto
reference player), as labelled by [SFZ-OP].

---

## 1. Lexical structure

### 1.1 Encoding and lines [I]

- Text, normally ASCII or UTF-8. Treat the file as UTF-8; on invalid UTF-8, fall back to
  Latin-1 for the whole file and note it in the import report [PH].
- Line endings LF, CRLF or CR are all accepted [I].
- Header names and opcode names are lowercase in the reference; match them
  case-sensitively and report a mixed-case name as unknown [PH]. Values that are keywords
  (`attack`, `loop_continuous`, `lpf_2p` …) are matched case-insensitively [PH].

### 1.2 Tokens

- **Comments:** `//` to end of line [S, SFZ-BASICS]. Block comments `/* … */` are accepted
  by common players [I]; the importer accepts them.
- **Header:** `<name>` [S]. Headers do not nest; a header ends where the next header of any
  kind begins [S, SFZ-BASICS].
- **Opcode:** `name=value` with no space around `=` in normal files. Accept optional
  whitespace around `=` [PH].
- **Separators:** any whitespace (spaces, tabs, newlines) separates opcodes; several opcodes
  may share a line, and one opcode per line is equivalent [S, SFZ-BASICS].
- **Directives:** `#define` and `#include` (section 2) at any point in the token stream [S].

### 1.3 Values that may contain spaces [I]

For these opcodes the value runs to the end of the line, or up to the start of the next
`name=` token on the same line, or a `//` comment, or a `<` header, whichever is first;
trailing whitespace is trimmed:

`sample`, `default_path`, `label_ccN`, `label_keyN`, `label_outputN`, `sw_label`,
`global_label`, `master_label`, `group_label`, `region_label`, `image`, `tdfir_impulse`.

All other values end at the first whitespace. "Next `name=` token" means a run of
characters `[A-Za-z0-9_]+` immediately followed by `=`, preceded by whitespace.

### 1.4 Number values [I]

- Integers may carry a sign. Floats use `.` as decimal point; exponents are not used in
  practice but are accepted.
- An integer opcode given a float value is truncated toward zero, with a report note [PH].
- An out-of-range value is clamped to the range in the tables and reported [PH].
- A value that does not parse leaves the opcode unset and is reported [PH].

---

## 2. Preprocessor

### 2.1 `#define` [S, SFZ-DEF]

- Syntax: `#define $NAME value`. The variable name starts with `$` [S]. Names are
  `$[A-Za-z0-9_]+` [I].
- The value is the next whitespace-delimited token [I].
- Every later occurrence of `$NAME` in an opcode **value** (and in an opcode **name**, e.g.
  `amplitude_oncc$CC=100`) is replaced textually [S for values; I for names].
- Redefining a variable later in the file changes it for text after the new `#define` [I].
  The reference warns that some players (ARIA) handle redefinition between includes
  poorly [S]; Patternhaus applies the straightforward reading: the value in force at the
  point of use [PH].
- Substitution happens before value parsing, so a defined value may be a note name or a
  number [I].
- Replace the longest matching name first, so `$VEL` does not eat the start of `$VELHI` [PH].
- An undefined `$NAME` is left as literal text (it will then fail to parse) and reported [PH].

### 2.2 `#include` [S, SFZ-INC]

- Syntax: `#include "relative/path.sfz"`. The path is in **double quotes** [S].
- The included file's tokens are inserted at the directive's position, as if pasted [S].
- Includes may nest [S]. Detect cycles and stop with an error naming the chain [PH];
  also cap the depth at 32 [PH].
- **Relative include paths resolve against the folder of the top-level (main) `.sfz`
  file**, not the folder of the file that contains the directive [S].
- `.sfzh` is the conventional extension for included fragments; it behaves exactly like
  `.sfz` [S].
- `#define`s made inside an included file stay in effect after it [I].
- A missing include file is an import error for that file only: report it and continue
  with the rest [PH].

### 2.3 Order of processing [I]

1. Strip comments.
2. Expand `#include` recursively.
3. Apply `#define` substitution in stream order.
4. Tokenize headers and opcodes, then build the hierarchy (section 3).

---

## 3. Headers and inheritance

### 3.1 Headers [S, SFZ-H]

| Header | Version | Role |
|---|---|---|
| `<control>` | v2 | file-wide loading settings: `default_path`, `note_offset`, `octave_offset`, `set_ccN`, `set_hdccN`, `label_ccN`, `#define`s may sit near it |
| `<global>` | v2 | defaults for every region below it |
| `<master>` | ARIA | defaults between global and group |
| `<group>` | v1 | defaults for the regions that follow |
| `<region>` | v1 | one playable zone |
| `<curve>` | v2 | a CC response curve (3.5) |
| `<effect>` | v2 | an effect on a bus (section 11) |
| `<midi>` | ARIA | MIDI-processing settings; keep as data, skip [PH] |
| `<sample>` | v2 | an embedded sample (base64 data in the file); skip and report [PH] |

### 3.2 Inheritance [S for the chain; I for the reset rules]

The chain is **global → master → group → region**, each level overriding the one above
[S]. Rules for building it from the token stream [I]:

- Keep four "current defaults" maps: global, master, group, and the region being built.
- `<global>`: start a new global map (empty), and clear master and group maps.
- `<master>`: start a new master map, and clear the group map.
- `<group>`: start a new group map.
- `<region>`: start a new region map; when it ends (next header or end of input), emit a
  region whose opcodes are the merge **global ∪ master ∪ group ∪ region**, later levels
  winning per opcode name.
- Opcodes before any header apply as if under an implicit `<global>` [PH].
- Opcodes after `<control>` go to the control map, which is **not** part of the merge;
  control settings apply from that point in the file onward and a later `<control>`
  replaces them [I].
- A region is emitted even if it has no `sample` (it is then reported and skipped) [PH].
- `<curve>` and `<effect>` blocks do not affect region inheritance [I].

**Merging is per opcode name**, so `amplitude_oncc1` at group level and
`amplitude_oncc7` at region level both apply. Some opcodes interact (`key` sets three
others, 4.1); expand them into their parts **at the level where they appear**, before
merging [I].

### 3.3 Note names and offsets

- A key value may be a number 0–127 or a **note name**: letter `A`–`G` (either case),
  optional accidental `#` or `b`, then an octave number which may be negative (`c-1`)
  [S for the range C-1 to G9, SFZ-SW; I for grammar].
- **C4 = 60** (so C-1 = 0, G9 = 127) [S, SFZ-BASICS]. Formula: `12 × (octave + 1) +
  semitone(letter) + accidental`, where C=0, D=2, E=4, F=5, G=7, A=9, B=11, `#` = +1,
  `b` = −1 [I].
- `octave_offset` (−10…10, default 0) adds 12 × value to every key-valued opcode parsed
  after it; `note_offset` (−127…127, default 0) adds its value [S, v2]. Both are
  `<control>` opcodes. Apply them to: `key`, `lokey`, `hikey`, `pitch_keycenter`,
  `sw_*` key opcodes, `xf*_lokey/hikey`, `amp_keycenter`, `fil_keycenter`,
  `pan_keycenter` [I]. `sw_note_offset` and `sw_octave_offset` (ARIA) apply the same way,
  but only to the `sw_*` opcodes [S].
- After offsets, clamp to 0–127.

### 3.4 Sample path resolution [I unless marked]

- `sample` is relative to the folder of the main `.sfz` file, with `default_path`
  (control) prefixed when set [S for default_path's purpose].
- Both `/` and `\` are path separators; normalise to `/` [I].
- Paths are tried exactly first, then case-insensitively per path component (libraries
  built on Windows often differ in case) [PH].
- Absolute paths (a drive letter or a leading `/`) are used as they are [I].
- Built-in generators: values starting with `*` name a synthetic source: `*sine`,
  `*saw`, `*square`, `*triangle`, `*noise`, `*silence` [D, sfzformat `sample` page].
  M3 supports `*silence` and `*sine`; the others are reported as skipped [PH].
- Sample files: WAV and FLAC must load; AIFF and Ogg should load if `ph-formats` reads
  them [PH].

### 3.5 Curves [S, SFZ-CURVE]

- `<curve>` with `curve_index=N` (custom curves 7–254 in ARIA) and points `v000`…`v127`
  (each a float).
- Unset points are linearly interpolated between the nearest set points [S].
- Missing endpoints: `v000 = 0`, `v127 = 1` [S].
- Built-in curves, not overridable [S]:

| Index | Shape |
|---|---|
| 0 | linear 0 → 1 (used when no curve is named) |
| 1 | linear −1 → 1 (bipolar) |
| 2 | linear 1 → 0 |
| 3 | linear 1 → −1 |
| 4 | concave 0 → 1 (exact shape not given: use x² [PH]) |
| 5 | crossfade-in power curve: √x [I] |
| 6 | crossfade-out power curve: √(1−x) [I] |

- A curve is used by `<target>_curveccN=<curve index>` paired with `<target>_onccN`
  [S]. The modulation amount is `depth × curve(cc/127)`.

---

## 4. Region mapping and triggering

### 4.1 Key and velocity

| Opcode | Ver | Type | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `lokey` / `hikey` | v1 | key | 0–127 | 0 / 127 | key range, inclusive [S] | must |
| `key` | v1 | key | 0–127 | — | sets `lokey`, `hikey` **and** `pitch_keycenter` to the value [S] | must |
| `lovel` / `hivel` | v1 | int | 1–127 | 1 / 127 | velocity range, inclusive [S] | must |
| `lochan` / `hichan` | v1 | int | 1–16 | 1 / 16 | MIDI channel range [S] | must (channel 1 when unrouted) |

`lokey=-1` (ARIA) makes a region respond to no key; such regions are only reached through
`on_loccN` triggers [I].

### 4.2 Trigger modes [S, SFZ-TRIG]

`trigger` (string, default `attack`):

| Value | Ver | Plays on |
|---|---|---|
| `attack` | v1 | note-on |
| `release` | v1 | note-off; if sustain (CC64) is down at note-off, when the pedal is released. Uses the velocity of the matching note-on |
| `first` | v1 | note-on only when no other note is held on the channel (first note of a phrase) |
| `legato` | v1 | note-on only when at least one other note is held (later notes of a phrase) |
| `release_key` | v2 | note-off, ignoring the sustain pedal |

Engine rules:
- Release regions play as one-shots (loop mode `one_shot` unless `loop_mode` is set
  explicitly to something else; ARIA forces one-shot) [S].
- Whether a release region needs a still-sounding attack region with the same key and
  velocity range differs between players [S]. **[PH]:** a release region plays whenever
  its own conditions match at note-off; `rt_dead=on` is accepted and ignored.
- Release level: `rt_decay` (v1, 0–200 dB/s, default 0) lowers the release region's
  volume by `rt_decay × seconds the note was held` [S for unit; I for formula].
  ARIA's `rt_decayN` / `rt_decayN_time` multi-segment form is skipped [PH].
- "Held" for `first` / `legato` counts keys physically down on the same MIDI channel,
  not sounding voices [I].
- Regions using `on_loccN`/`on_hiccN` are CC-triggered and should have no `trigger` set
  [S]; see 4.6.

### 4.3 Round robins and random selection

| Opcode | Ver | Type | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `seq_length` | v1 | int | 1–100 | 1 | length of the round-robin cycle [S] | must |
| `seq_position` | v1 | int | 1–100 | 1 | the step of the cycle at which this region plays [S] | must |
| `lorand` / `hirand` | v1 | float | 0–1 | 0 / 1 | region plays when a random number r drawn per note-on falls in `lorand ≤ r < hirand` (`hirand = 1` includes 1) [S for range; I for the bound rule] | must |

**Sequence counter [I] [PH]:** players differ on whose counter advances. Patternhaus keeps
one counter per region, starting at 1. On each note-on that matches the region's other
conditions (key, velocity, channel, CC, keyswitch, trigger), the region plays when
`counter == seq_position`, then the counter advances (`counter = counter mod seq_length
+ 1`). Because all round-robin siblings match the same notes, their counters move in step.
Counters reset when the instrument loads and when the engine resets.

**Random number [PH]:** one value per note-on event, shared by all regions evaluated for
that event (so `lorand`/`hirand` partitions are exclusive), drawn from the seeded
per-render generator (M3 criterion 5).

### 4.4 Keyswitches (`sw_*`) [S, SFZ-SW; others from their opcode pages]

| Opcode | Ver | Type | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `sw_lokey` / `sw_hikey` | v1 | key | 0–127 | 0 / 127 | the keyswitch key range: notes in it update the "last keyswitch" state | must |
| `sw_last` | v1 | key | 0–127 | unset | region plays only if the most recent key pressed **inside the sw range** equals this; sticky until another keyswitch | must |
| `sw_lolast` / `sw_hilast` | ARIA | key | 0–127 | unset | as `sw_last`, but matches a range of keyswitches | must |
| `sw_down` | v1 | key | 0–127 | unset | region plays only while this key is held down | must |
| `sw_up` | v1 | key | 0–127 | unset | region plays only while this key is **not** held | must |
| `sw_previous` | v1 | key | 0–127 | unset | region plays only if the previous note (any key) was this key | must |
| `sw_vel` | v1 | string | `current`, `previous` | `current` | which velocity the region uses for its velocity range: this note's or the previous note's | may skip |
| `sw_default` | v2 | key | 0–127 | unset | the keyswitch assumed active before any keyswitch is played | must |
| `sw_label` | ARIA | string | — | — | name of the articulation (shown on the keyboard) | must (as data) |

Rules:
- Without `sw_default` the "last keyswitch" state starts empty, so `sw_last` regions are
  silent until a keyswitch is played [S]. With several different `sw_default` values in
  one file, the last one parsed wins [PH].
- The `sw_lokey`/`sw_hikey` range of the region being evaluated decides whether a note is a
  keyswitch. **[PH]:** the engine builds one keyswitch state per instrument: the union of
  all regions' sw ranges defines the keyswitch keys; the last key pressed in that union is
  the "last keyswitch".
- A keyswitch key plays sound only if some region maps it (no implicit muting) [I].
  Libraries normally put keyswitches outside the playable range.

### 4.5 MIDI conditions

| Opcode | Ver | Type | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `loccN` / `hiccN` | v1 | int | 0–127 | 0 / 127 | region plays only if CC N's current value is in range (N 0–127; ARIA allows extended numbers, 4.6) | must |
| `lohdccN` / `hihdccN` | ARIA | float | 0–1 | 0 / 1 | the same with normalised CC values | must (same logic) |
| `lobend` / `hibend` | v1 | int | −8192–8192 | −8192 / 8192 | pitch-bend condition | must |
| `lochanaft` / `hichanaft` | v1 | int | 0–127 | 0 / 127 | channel aftertouch condition | must |
| `lopolyaft` / `hipolyaft` | v1 | int | 0–127 | 0 / 127 | poly aftertouch condition | may skip |
| `loprog` / `hiprog` | v2 | int | 0–127 | 0 / 127 | program change condition | may skip |
| `lobpm` / `hibpm` | v1 | float | 0–500 | 0 / 500 | host tempo condition | may skip |
| `lotimer` / `hitimer` | v2 | float | s | 0 / max | time since last note condition | may skip |
| `sustain_sw`, `sostenuto_sw` | v2 | on/off | — | on | whether the region obeys sustain/sostenuto | must (`sustain_sw`) |
| `sustain_cc` / `sustain_lo` | ARIA | int | 0–127 | 64 / 1 | which CC is the sustain pedal, and its "down" threshold | must |
| `set_ccN` | v2 | int | 0–127 | 0 | initial value of CC N at load (`<control>`) | must |
| `set_hdccN` | ARIA | float | 0–1 | 0 | the same, normalised | must |

**CC initial values [I]:** CCs not set by `set_ccN` start at 0, except CC 7 (volume) at 100,
CC 10 (pan) and CC 11 (expression) at 64 and 127 respectively [I: common MIDI defaults;
the importer may report that these are assumptions]. Patternhaus default: everything 0
except CC 7 = 100, CC 10 = 64, CC 11 = 127 [PH].

### 4.6 CC-triggered regions [S]

- `on_loccN` / `on_hiccN` (v1, int 0–127, default −1 = unset): the region plays when CC N
  **changes to** a value inside the range (the transition from outside to inside, or any
  new value inside? players vary: **[PH]** any CC event whose new value is inside the
  range triggers the region once).
- The note used for pitch is `pitch_keycenter` (no key is pressed); velocity is 127 [I].
- `on_lohdccN` / `on_hihdccN` (ARIA) are the normalised form.
- `stop_loccN` / `stop_hiccN` (v2): stop the region's voices when CC N enters the range;
  may skip.

### 4.7 Extended CC numbers [S, ARIA]

ARIA defines pseudo-CC numbers above 127, usable anywhere a CC number is: 128 pitch bend,
129 channel aftertouch, 130 poly aftertouch, 131 note-on velocity, 132 note-off velocity,
133 note number, 134 gate, 135 unipolar random, 136 bipolar random, 137 alternate, 140
keydelta, 141 absolute keydelta [D, sfzformat "extended CCs" page]. M3 must support 128,
129, 131, 133, 135 and 136 as modulation sources; the rest may be skipped.

---

## 5. Sample playback

| Opcode | Ver | Type | Range | Unit | Default | Meaning | M3 |
|---|---|---|---|---|---|---|---|
| `sample` | v1 | path | — | — | — | the sample file (3.4) | must |
| `offset` | v1 | int | 0–2³² | frames | 0 | start frame | must |
| `offset_random` | v1 | int | 0–2³² | frames | 0 | a random amount 0…value added to `offset` per note | must |
| `offset_onccN` (`offset_ccN`) | v1/v2 | int | 0–2³² | frames | 0 | added to `offset` × cc/127 | must |
| `end` | v1 | int | 0–2³² | frames | file end | last frame played, **inclusive** [I]. `end=0` or `end=-1` makes the region silent [D] | must |
| `count` | v1 | int | 0–2³² | — | 0 | play the sample this many times then stop; forces one-shot [S, loop_mode page] | may skip (approximate as one-shot) |
| `delay` | v1 | float | 0–100 | s | 0 | delay from trigger to start | must |
| `delay_random` | v1 | float | 0–100 | s | 0 | random extra delay 0…value | must |
| `delay_onccN` (`delay_ccN`) | v1/v2 | float | −100–100 | s | 0 | added × cc/127 | may skip |
| `direction` | v2 | string | `forward`, `reverse` | — | `forward` | play backward from `end` to `offset` | must |
| `sample_fadeout` | v2 | float | s | — | 0 | fade at sample end | may skip |
| `sync_beats`, `sync_offset`, `delay_beats`, `stop_beats` | v1/v2 | float | beats | — | 0 | tempo-synced starts | may skip |
| `md5` | v2 | string | — | — | — | sample checksum | skip (keep as data) |
| `waveguide` | v2 | on/off | — | — | off | | skip |
| `load_mode`, `load_start`, `load_end`, `sample_quality`, `image` | v2 | | | | | loader hints and UI image | skip (keep `image` as data) |

### 5.1 Loops [S, SFZ-LOOP; others from their pages]

| Opcode | Ver | Type | Values / range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `loop_mode` (alias `loopmode`) | v1 | string | `no_loop`, `one_shot`, `loop_continuous`, `loop_sustain` | from the sample file: `loop_continuous` if the file has a loop, else `no_loop` [S] | see below | must |
| `loop_start` (alias `loopstart`) | v1 | int | 0–2³² frames | file's loop start, else 0 | first frame of the loop | must |
| `loop_end` (alias `loopend`) | v1 | int | 0–2³² frames | file's loop end, else last frame | last frame of the loop, **inclusive** [I] | must |
| `loop_type` | v2 | string | `forward`, `backward`, `alternate` | `forward` | direction inside the loop; `alternate` = ping-pong | must |
| `loop_crossfade` | v2 | float | 0–? s | 0 | crossfade length at the loop seam | must |
| `loop_count` | v2 | int | — | unset (infinite) | number of loop repetitions | may skip |
| `loop_tune` | v2 | float | cents | 0 | tuning applied inside the loop | skip |
| `loop_startccN`, `loop_lengthccN` | v2 | int | frames | 0 | CC-modulated loop | skip |
| `loop_end_offset` | ARIA | int | frames | 0 | | skip |

Modes:
- `no_loop`: play `offset`…`end`; note-off starts the release envelope [S].
- `one_shot`: play to the end regardless of note-off; the amp envelope release is not
  entered on note-off [S].
- `loop_continuous`: loop `loop_start`…`loop_end` until the voice ends, including during
  release [S].
- `loop_sustain`: loop while the note is held (key or sustain pedal); on release, play on
  from the loop position to `end` [S for looping while held; I for "play on"].
- Sample-file loop data: WAV `smpl` chunk loops (first loop only), FLAC/AIFF loop
  metadata if `ph-formats` exposes it [I].
- **ARIA quirk** [S]: with `loop_continuous`/`loop_sustain` set, no `loop_end`, and no loop
  in the file, the whole sample loops. Patternhaus follows this [PH].

---

## 6. Amplifier

| Opcode | Ver | Type | Range | Unit | Default | Meaning | M3 |
|---|---|---|---|---|---|---|---|
| `volume` | v1 | float | −144–6 | dB | 0 | region gain | must |
| `amplitude` | ARIA | float | 0–100 | % | 100 | linear gain | must |
| `pan` | v1 | float | −100–100 | % | 0 | stereo pan, −100 left | must |
| `width` | v1 | float | −100–100 | % | 100 | stereo width; 0 = mono, negative = channels swapped | must |
| `position` | v1 | float | −100–100 | % | 0 | stereo position of a wide sample | must |
| `amp_keycenter` | v1 | key | 0–127 | — | 60 | centre for key tracking | must |
| `amp_keytrack` | v1 | float | −96–12 | dB/key | 0 | gain change per key from the centre | must |
| `amp_veltrack` | v1 | float | −100–100 | % | 100 | how much velocity lowers gain | must |
| `amp_velcurve_N` | v1 | float | 0–1 | — | — | a point of a custom velocity→gain curve at velocity N (1–127), linear between points; `amp_velcurve_127` defaults to 1 | must |
| `amp_random` | v1 | float | 0–24 | dB | 0 | random gain per note (0…−value) | must |
| `gain_onccN` (`volume_onccN`, v1 `gain_ccN`) | v1/v2 | float | −144–48 | dB | 0 | gain × cc/127 | must |
| `amplitude_onccN` | ARIA | float | 0–100 | % | 0 | linear gain modulation | must |
| `pan_onccN`, `width_onccN`, `position_onccN` | v2/ARIA | float | −200–200 | % | 0 | | must (`pan`), may skip others |
| `*_curveccN`, `*_smoothccN`, `*_stepccN` | v2 | | | | | curve index, smoothing (ms), quantise steps for the matching `_onccN` | must (`curvecc`); may skip smooth/step |
| `pan_law` | ARIA | string | `balance`, `mma` | | | | may skip |
| `global_volume`, `master_volume`, `group_volume` and `*_amplitude` | ARIA | | | | | per-level gains that multiply, not override | must |
| `phase` | v2 | string | `normal`, `invert` | | `normal` | polarity | may skip |
| `output` | v1 | int | 0–1024 | — | 0 | output pair index (multi-out) | must |

**Velocity to gain [I]:** with no `amp_velcurve_N`, the gain from velocity v (1–127) is
`g = 1 − (t × (1 − (v/127)²))` where t = `amp_veltrack/100` (negative t inverts so that soft
notes are loud: use `(1 − v/127)` in place of `v/127`). With custom points, the curve
replaces `(v/127)²` and `amp_veltrack` still scales it [I].

### 6.1 Crossfades [S, SFZ-XF]

| Opcode | Ver | Range | Default | M3 |
|---|---|---|---|---|
| `xfin_lokey` / `xfin_hikey` | v1 | 0–127 | 0 / 0 | must |
| `xfout_lokey` / `xfout_hikey` | v1 | 0–127 | 127 / 127 | must |
| `xfin_lovel` / `xfin_hivel` | v1 | 0–127 | 0 / 0 | must |
| `xfout_lovel` / `xfout_hivel` | v1 | 0–127 | 127 / 127 | must |
| `xfin_loccN` / `xfin_hiccN` | v1 | 0–127 | 0 / 0 | must |
| `xfout_loccN` / `xfout_hiccN` | v1 | 0–127 | 127 / 127 | must |
| `xf_keycurve`, `xf_velcurve`, `xf_cccurve` | v1 | `gain`, `power` | `power` | must |

[S for `xfin_loccN` default and the at-or-below / at-or-above rule; I for the other defaults,
chosen so that an unset crossfade has no effect.]

Gain for one crossfade dimension with input x (key, velocity or CC value):
- fade-in: x ≤ lo → 0; x ≥ hi → 1; between: p = (x − lo) / (hi − lo) [S for ends; I for the
  linear position].
- fade-out: x ≤ lo → 1; x ≥ hi → 0; between: p = 1 − (x − lo)/(hi − lo).
- curve `gain`: factor = p; curve `power`: factor = √p [I: "equal power" per S].
- Region gain is the product of all its crossfade factors [I].
- CC crossfades follow the CC **continuously** while the voice plays (a mod-wheel dynamics
  crossfade) [I]; key and velocity crossfades are fixed at note-on.
- Regions at zero crossfade gain are still triggered (so that moving the CC brings them in)
  unless `loccN`/`hiccN` exclude them [S].

---

## 7. Pitch

| Opcode | Ver | Type | Range | Unit | Default | Meaning | M3 |
|---|---|---|---|---|---|---|---|
| `pitch_keycenter` | v1 | key | −127–127 (or `sample`) | — | 60 | the key at which the sample plays unshifted; `sample` = take the root from the file's `smpl` chunk | must |
| `pitch_keytrack` | v1 | int | −1200–1200 | cents/key | 100 | pitch change per key; 0 = fixed pitch (drums) | must |
| `tune` | v1 | int | −100–100 (ARIA: ±9600) | cents | 0 | fine tune | must |
| `transpose` | v1 | int | −127–127 | semitones | 0 | | must |
| `pitch_veltrack` | v1 | int | −9600–9600 | cents | 0 | pitch at velocity 127 relative to 0 | must |
| `pitch_random` | v1 | float | 0–9600 | cents | 0 | random detune ± value/2 [I] | must |
| `bend_up` / `bend_down` | v1 | int | −9600–9600 | cents | 200 / −200 | pitch-bend range | must |
| `bend_step` | v1 | int | 1–1200 | cents | 1 | bend quantisation | may skip |
| `bend_smooth` | v2 | float | 0–? | ms | 0 | | may skip |
| `pitch_onccN` (`tune_onccN`, v1 `pitch_ccN`) | v1/v2 | int | −9600–9600 | cents | 0 | pitch × cc/127 | must |
| `global_tune`, `master_tune`, `group_tune` | ARIA | | cents | 0 | add to `tune` | must |

**Playback ratio [I]:** semitone offset = `(key − pitch_keycenter) × pitch_keytrack/100 +
transpose + (tune + veltrack term + random + bend + modulation)/100`; ratio =
2^(offset/12) × (file rate / engine rate).

---

## 8. Envelope generators

### 8.1 `ampeg_*` (v1, DAHDSR) [S, opcode pages]

| Opcode | Range | Unit | Default | M3 |
|---|---|---|---|---|
| `ampeg_delay` | 0–100 | s | 0 | must |
| `ampeg_start` | 0–100 | % | 0 | must |
| `ampeg_attack` | 0–100 | s | 0 | must |
| `ampeg_hold` | 0–100 | s | 0 | must |
| `ampeg_decay` | 0–100 | s | 0 | must |
| `ampeg_sustain` | 0–100 | % | 100 | must |
| `ampeg_release` | 0–100 | s | 0.001 [D, sfzformat ampeg_release page; some v1 players use 0] | must |
| `ampeg_vel2delay`, `…vel2attack`, `…vel2hold`, `…vel2decay`, `…vel2release` (added × velocity/127) | −100–100 | s | 0 | must |
| `ampeg_vel2sustain` (added × velocity/127) | −100–100 | % | 0 | must |
| `ampeg_delayccN`, `…attackccN`, … and `_onccN` forms (added × cc/127 at note-on) | −100–100 | s or % | 0 | may skip |
| `ampeg_attack_shape`, `…decay_shape`, `…release_shape` (ARIA; curve bend, 0 = linear) | float | — | 0 | may skip (approximate) |
| `ampeg_dynamic` (ARIA; re-evaluate CC terms while playing) | on/off | | off | skip |

Shape [I]: attack is linear in amplitude; decay and release are exponential (linear in dB)
in common players. **[PH]:** attack linear, decay and release exponential reaching −90 dB
at the stated time.

### 8.2 `fileg_*` and `pitcheg_*` (v1)

Same stages and ranges as 8.1, plus `fileg_depth` / `pitcheg_depth` (−12000–12000 cents,
default 0) [S]. The envelope output (0–1) × depth is added to cutoff (`fileg`) or pitch
(`pitcheg`), in cents. `*_vel2depth` (cents) adds × velocity/127. M3: must for `fileg_*`,
must for `pitcheg_*`.

### 8.3 `egN_*` flex envelopes (v2)

Multi-point envelopes: `egN_points`, `egN_timeX`, `egN_levelX` (−1…1), `egN_shapeX`,
`egN_sustain` (point index), `egN_loop`, routed by `egN_<target>` depth opcodes
(`egN_amplitude`, `egN_volume`, `egN_pan`, `egN_pitch`, `egN_cutoff`, `egN_resonance`, …)
and `egN_ampeg` (ARIA: this EG replaces the amp envelope) [S]. M3: **approximated**:
import the points into the modulation matrix as a generic multi-stage envelope if T2's
envelope supports points; otherwise skip and report [PH].

---

## 9. LFOs

### 9.1 Legacy LFOs (v1): `amplfo_*`, `fillfo_*`, `pitchlfo_*` [S]

Each family has: `_delay` (0–100 s, 0), `_fade` (0–100 s, 0: fade-in time), `_freq`
(0–20 Hz, 0), `_depth` (amp: −10–10 dB; fil and pitch: −1200–1200 cents; default 0),
`_depthccN`/`_depth_onccN`, `_depthchanaft`, `_freqccN`, `_freqchanaft` (and poly-AT forms).
Wave is a sine [I]. M3: must for `_delay`, `_fade`, `_freq`, `_depth`; may skip the
aftertouch forms.

### 9.2 Numbered LFOs (v2): `lfoN_*` [S, SFZ-LFOW and opcode pages]

| Opcode | Range | Unit | Default | Meaning | M3 |
|---|---|---|---|---|---|
| `lfoN_freq` | 0–100 | Hz | 0 | rate | must |
| `lfoN_delay` | 0–100 | s | 0 | | must |
| `lfoN_fade` | 0–100 | s | 0 | | must |
| `lfoN_phase` | 0–1 | cycles | 0 | start phase | must |
| `lfoN_wave` | see below | | 1 (sine) [S table; ARIA uses triangle] | | must for 0–7 |
| `lfoN_count` | int | | 0 = infinite | number of cycles | may skip |
| `lfoN_steps`, `lfoN_stepX` | | | | step sequencer LFO (wave 13) | may skip |
| `lfoN_smooth` | | | | | may skip |
| `lfoN_offset`, `lfoN_ratio`, `lfoN_scale` and `…X` sub-oscillators | ARIA | | | | skip |
| `lfoN_volume` | | dB | 0 | target depth | must |
| `lfoN_amplitude` | | % | 0 | | must |
| `lfoN_pan`, `lfoN_width` | | % | 0 | | must (`pan`) |
| `lfoN_pitch` | | cents | 0 | | must |
| `lfoN_cutoff`, `lfoN_resonance` | | cents, dB | 0 | | must |
| `lfoN_freq_lfoX`, `lfoN_depth_lfoX`, `lfoN_depthadd_lfoX` | | | | LFO-to-LFO | may skip |
| `lfoN_eqXfreq`, `lfoN_decim`, `lfoN_bitred`, `lfoN_noise*`, `lfoN_drive` | | | | | skip |

Each target has `_onccN` depth modulation forms (e.g. `lfo1_pitch_oncc1`, the classic
mod-wheel vibrato) [S]. M3 must support `_onccN` on `pitch`, `volume`, `cutoff`.

Waves [S, SFZ-LFOW]: 0 triangle, 1 sine, 2 pulse 75 %, 3 square, 4 pulse 25 %, 5 pulse
12.5 %, 6 saw rising, 7 saw falling, 12 (ARIA) random sample-and-hold, two values per
period, 13 (ARIA) stepped (uses `lfoN_steps`). 8–11 are ARIA-reserved or product-specific;
−1 is deprecated. Unsupported waves fall back to sine with a report entry [PH].

---

## 10. Filters

| Opcode | Ver | Range | Unit | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `cutoff` | v1 | 0–Nyquist | Hz | unset = **no filter** | filter cutoff | must |
| `resonance` | v1 | 0–40 | dB | 0 | | must |
| `fil_type` (alias `filtype`) | v1 | see below | | `lpf_2p` | | must for the v1 types, approximated for others |
| `fil_keytrack` | v1 | 0–1200 | cents/key | 0 | | must |
| `fil_keycenter` | v1 | 0–127 | key | 60 | | must |
| `fil_veltrack` | v1 | −9600–9600 | cents | 0 | cutoff shift at velocity 127 | must |
| `fil_random` | v1 | 0–9600 | cents | 0 | | must |
| `cutoff_onccN` (`cutoff_ccN`) | v1/v2 | −9600–9600 | cents | 0 | | must |
| `resonance_onccN` | v2 | −40–40 | dB | 0 | | must |
| `cutoff_chanaft`, `cutoff_polyaft` | v1 | −9600–9600 | cents | 0 | | may skip |
| `cutoff2`, `resonance2`, `fil2_type`, `fil2_keytrack`, … | v2 | as above | | | second filter in series | must (same code) |
| `fil_gain`, `fil2_gain` | ARIA | dB | | 0 | gain for shelf/peak types | may skip |
| `eqN_freq`, `eqN_gain`, `eqN_bw`, `eqN_type` (N = 1–3) | v1/v2 | Hz, dB, octaves | | 0 dB | three EQ bands per region | approximate (a peaking band per entry) |

`fil_type` values [S, SFZ-FIL]:
- v1: `lpf_1p`, `hpf_1p`, `lpf_2p`, `hpf_2p`, `bpf_2p`, `brf_2p`;
- v2: `bpf_1p`, `brf_1p`, `apf_1p`, `lpf_2p_sv`, `hpf_2p_sv`, `bpf_2p_sv`, `brf_2p_sv`,
  `pkf_2p`, `lpf_4p`, `hpf_4p`, `lpf_6p`, `hpf_6p`, `comb`, `pink`;
- ARIA: `lsh`, `hsh`, `peq` (use `fil_gain`).

Mapping [PH]: `_1p` = 6 dB/oct, `_2p` = 12, `_4p` = 24, `_6p` = 36; `_sv` variants map to
the same response; `comb`, `pink`, `apf_1p` are skipped (the filter is bypassed and
reported).

**Cutoff [I]:** effective cutoff in Hz = `cutoff × 2^(cents/1200)` where cents = keytrack ×
(key − keycenter) + veltrack × velocity/127 + random + envelope + LFO + CC terms; clamp to
the engine's range.

---

## 11. Voice management

| Opcode | Ver | Type | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|
| `group` (alias `polyphony_group`) | v1 | int | any int | 0 | exclusive-group id used by `off_by` (not the `<group>` header) | must |
| `off_by` | v1 | int | any int | unset | when a region of group *off_by* starts, voices of this region stop | must |
| `off_mode` | v1 | string | `fast`, `normal`, `time` | `fast` | how choked voices end: `fast` ≈ a few ms, `normal` = their release envelope, `time` = `off_time` | must |
| `off_time` | ARIA | float | s | 0.006 [I] | fade time for `off_mode=time` | must |
| `off_shape`, `off_curve` | ARIA | | | | | skip |
| `polyphony` | v2 | int | 0–? | unlimited | voice limit for this region or, at group level, the group | must |
| `note_polyphony` | v2 | int | | unlimited | voices per key | must |
| `note_selfmask` | v2 | on/off | | on | whether a new note masks older notes of the same key under `note_polyphony` | may skip |
| `polyphony_stealing` | ARIA | | | | | skip |
| `rt_dead` | v2 | on/off | | off | release needs a live attack | accept, ignore [PH, 4.2] |

[S for opcode names and versions; I for exact choke timings.] **[PH]:** a voice choked with
`off_mode=fast` fades out over 6 ms.

---

## 12. `<effect>` and buses

`<effect>` blocks hold `type` (e.g. `lofi`, `filter`, `eq`, `apan`, `comp`, `gate`,
`disto`, `phaser`, `delay`, `reverb`, `strings`, `static`, `tdfir`, `fverb`, and
vendor-specific types), `bus` (`main`, `aux1`–`aux8`, `fx1`–`fx4`), `dsp_order`, routing
amounts `fxNtomain`, `fxNtomix`, and type-specific parameters (`reverb_*`, `delay_*`,
`phaser_*`, `disto_*`, `comp_*`, `gate_*`, `eq_*`, `filter_*`, `bitred`, `decim`, …)
[S, SFZ-OP]. Regions send to effect buses with `effect1`–`effect4` (0–100 %, v1/v2) [S].

M3 **[PH]:** keep every `<effect>` block as data in the native instrument (so nothing is
lost). Map `reverb`, `delay`, `comp`, `eq`, `filter`, `phaser` and `disto` to the nearest
built-in Patternhaus effects on the instrument's output as **approximated**; skip and
report all other types and all vendor-specific parameters.

---

## 13. Instrument-level and UI opcodes

- `label_ccN` (ARIA, string): CC name for the UI; keep as data and show [PH].
- `label_keyN` (sfizz), `label_outputN` (Calfbox): keep as data.
- `global_label`, `master_label`, `group_label`, `region_label` (ARIA): names; use for group
  names in the zone map.
- `hint_*` (ARIA): player hints; skip.
- `varNN_*` (ARIA): variable modulators; skip and report.
- `<midi>` header opcodes: skip.
- `oscillator*` (wavetable): skip and report.
- `noise_*`, `decim`, `bitred` as region opcodes: skip.

---

## 14. Opcode aliases [S, SFZ-OP lists aliases on each opcode page]

The importer normalises these before merging (old name → canonical):
`loopmode` → `loop_mode`, `loopstart` → `loop_start`, `loopend` → `loop_end`,
`filtype` → `fil_type`, `bendup` → `bend_up`, `benddown` → `bend_down`, `bendstep` →
`bend_step`, `polyphony_group` → `group`, `gain_ccN` / `volume_onccN` → `gain_onccN`,
`pitch_ccN` / `tune_onccN` / `tune_ccN` → `pitch_onccN`, `cutoff_ccN` → `cutoff_onccN`,
`offset_ccN` → `offset_onccN`, `delay_ccN` → `delay_onccN`, `ampeg_<stage>ccN` →
`ampeg_<stage>_onccN`, `amplfo_depthccN` → `amplfo_depth_onccN` (same for `fillfo_`,
`pitchlfo_`), `amp_*` ↔ `ampeg_*` are **not** aliases [I]. If both an alias and its
canonical name appear at the same level, the later one wins [PH].

---

## 15. Unknown opcodes and the import report

- Every opcode not in this document's tables is **listed by name with its count** in the
  report, never dropped silently (M3 scope rule).
- Every opcode in a table marked "may skip" or "skip" that actually occurs is listed as
  skipped, with its count.
- Opcodes marked "approximate" are listed under approximations with what was done.
- The report also lists: missing samples, regions without a sample, unresolved `$`
  variables, include errors, clamped values.

---

## 16. Version summary

- **v1:** `<group>`, `<region>`; key/velocity/channel/CC/bend/aftertouch/bpm/random/sequence
  conditions; `sw_lokey` … `sw_vel`; `trigger` (`attack`, `release`, `first`, `legato`);
  sample playback, loops (`loop_mode`, `loop_start`, `loop_end`); `volume`, `pan`, `width`,
  `position`, crossfades; pitch opcodes; filter v1 types; `ampeg_*`, `fileg_*`, `pitcheg_*`;
  `amplfo_*`, `fillfo_*`, `pitchlfo_*`; `eq1`–`eq3`; `group`, `off_by`, `off_mode`, `output`.
- **v2:** `<control>`, `<global>`, `<curve>`, `<effect>`, `<sample>`; `#define`;
  `default_path`, `note_offset`, `octave_offset`, `set_ccN`; `release_key`;
  `sw_default`; `_onccN` forms; `lfoN_*`, `egN_*`; second filter; more filter types;
  `loop_type`, `loop_crossfade`, `direction`; `polyphony`, `note_polyphony`.
- **ARIA:** `<master>`, `<midi>`; `#include` (listed as ARIA by SFZ-OP); `sw_lolast`/`sw_hilast`,
  `sw_label`, `label_ccN`; HD CCs; extended CCs; `amplitude`; per-level volume/amplitude/tune;
  curve indices 0–6; LFO waves 12–13; `off_time`; filter types `lsh`/`hsh`/`peq`.

---

## 17. Unknowns and player differences

1. **Round-robin counter scope:** per region, per group header or per key. [PH] chooses per
   region (4.3). Fixture F-SFZ-04 checks the engine is self-consistent; behaviour vs ARIA
   needs an owner listening test.
2. **Release-without-attack** rules (4.2).
3. **Default LFO wave** (sine per the table, triangle in ARIA for `lfoN`).
4. **`end` / `loop_end` inclusivity**: [I] inclusive; one-frame differences are inaudible
   except in very short loops. A generated fixture with a 2-frame loop decides our engine's
   reading, not the format's.
5. **Exact envelope and choke shapes** (8.1, 11): players differ; ours is [PH].
6. **Built-in curve 4** shape.
7. **CC initial values** when no `set_ccN` (4.5).
8. **`on_loccN` re-trigger rule** (4.6).

---

## 18. Fixture plan (generated, never copied)

Fixtures are written by test code in `ph-sampler-formats` (text) with samples rendered by
`tools/soundgen` (sine bursts or tagged tones whose pitch or a DC "ID level" identifies
which region played). Each fixture renders to a reference.

| Id | Content | Exercises |
|---|---|---|
| F-SFZ-01 basic | 3 regions, `lokey`/`hikey`/`pitch_keycenter`, one with `key=` | mapping, `key` expansion, pitch ratio |
| F-SFZ-02 hierarchy | `<global>` `<master>` `<group>` `<region>` with overrides at each level, two `<master>` blocks | inheritance and resets (3.2) |
| F-SFZ-03 velocity | 3 velocity layers, `amp_veltrack`, `amp_velcurve_N`, `xfin_/xfout_lovel` | velocity mapping and crossfades |
| F-SFZ-04 round robin | `seq_length=3` siblings; a second set using `lorand`/`hirand` | sequence counter, seeded random |
| F-SFZ-05 triggers | attack + `trigger=release` with `rt_decay`, plus `first`/`legato` regions | triggers |
| F-SFZ-06 keyswitch | two articulations via `sw_lokey`/`sw_hikey`/`sw_last`/`sw_default`/`sw_label`, one `sw_down`, one `sw_previous` | keyswitches (acceptance test articulations) |
| F-SFZ-07 cc | `loccN`/`hiccN` layers; `xfin_loccN`/`xfout_loccN` on CC1 with `xf_cccurve=gain` and `power`; `set_cc1` | CC conditions and dynamics crossfade |
| F-SFZ-08 loops | WAV with `smpl` loop and no `loop_mode`; explicit `loop_sustain`, `loop_continuous`, `loop_type=alternate`, `loop_crossfade`, `one_shot`, `direction=reverse` | loops |
| F-SFZ-09 envelopes | `ampeg_*` all stages + `vel2*`; `fileg_*` with depth; `pitcheg_*` | envelopes |
| F-SFZ-10 lfo | `amplfo_*`, `pitchlfo_*`, `lfo1_wave` 0–7 to pitch, `lfo1_pitch_oncc1` | LFOs |
| F-SFZ-11 filter | `cutoff`, `resonance`, each v1 `fil_type`, `fil_keytrack`, `fil_veltrack`, `cutoff_oncc74` | filters |
| F-SFZ-12 preprocessor | `#define` (incl. `$VEL`/`$VELHI` prefix case, define inside include), nested `#include` from a subfolder that uses main-file-relative paths, `default_path`, `note_offset`, `octave_offset`, note names `c#4`, `Eb-1`, `G9` | section 2, 3.3, 3.4 |
| F-SFZ-13 voices | `group`/`off_by` hi-hat choke with each `off_mode`; `polyphony`, `note_polyphony` | voice management |
| F-SFZ-14 curves | `<curve>` with sparse points, `amplitude_oncc1` + `amplitude_curvecc1` | curves |
| F-SFZ-15 syntax | CRLF, tabs, block comments, `sample` with spaces and backslashes, case-mismatched sample path, opcode alias names, unknown opcodes, out-of-range values | lexer, aliases, report |
| F-SFZ-16 effects | `<effect>` reverb and delay, `effect1=` | kept as data, approximated |
| F-SFZ-17 acceptance | velocity layers, round robins, release triggers, two keyswitched articulations (M3 criterion 1) | end-to-end |
