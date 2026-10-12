# FL Studio project file (`.flp`): format specification

**Status:** M2/T4, first edition (2026-10-11). Written for the FLP importer (M2/T5), which
is implemented from this document alone.

**Clean-room statement.** This document states facts about the byte layout of `.flp` files.
They come from (a) byte-level analysis of the owner's own FL Studio projects (255 files,
read-only, never committed; aggregate numbers are in `flp-corpus-report.md`) and (b) public
documentation pages, cited below as hints. No parser source code (PyFLP's or any other) was
read, and FL Studio was not decompiled or disassembled. The wording is our own. Our analysis
tool, `tools/flp-probe`, was written from these observations.

**Evidence tags** used on every fact:

- **[C]** confirmed in corpus: seen in the files, consistent everywhere it was checked;
- **[D]** documented, not seen (or only seen with a single default value, so not testable);
- **[I]** inferred: a reading of the corpus that fits all the data but was not proven by a
  controlled change. The "Controlled test files" section lists the files that would settle each one.

**Cited sources** (all public documentation pages of the PyFLP project; reference pages only,
never its `_modules` source listings):

- [P-FMT] https://pyflp.readthedocs.io/en/latest/architecture/flp-format.html (chunks, event size classes)
- [P-EV] https://pyflp.readthedocs.io/en/latest/reference/events.html (event families)
- [P-PRJ] https://pyflp.readthedocs.io/en/latest/reference/project.html (project ids, tempo, PPQ, file formats)
- [P-CH] https://pyflp.readthedocs.io/en/latest/reference/channel/channel.html (channel ids, channel types, level ranges)
- [P-AUT] https://pyflp.readthedocs.io/en/latest/reference/channel/automation.html (automation points)
- [P-PAT] https://pyflp.readthedocs.io/en/latest/reference/patterns/pattern.html (pattern ids, note fields)
- [P-ARR] https://pyflp.readthedocs.io/en/latest/reference/arrangement/arrangement.html (arrangement ids, time signature)
- [P-PL] https://pyflp.readthedocs.io/en/latest/reference/arrangement/playlist.html (playlist item fields)
- [P-TRK] https://pyflp.readthedocs.io/en/latest/reference/arrangement/track.html (track ids and defaults)
- [P-MIX] https://pyflp.readthedocs.io/en/latest/reference/mixer/index.html (mixer ids, insert counts by version)
- [P-INS] https://pyflp.readthedocs.io/en/latest/reference/mixer/insert.html (insert ids, level ranges, EQ)
- [P-SLOT] https://pyflp.readthedocs.io/en/latest/reference/mixer/slot.html (slot id, mix range)
- [P-VST] https://pyflp.readthedocs.io/en/latest/reference/plugins/vst.html (VST identity fields)

Conventions: all integers are little-endian. `u8/u16/u32/i32` are unsigned/signed integers of
that width, `f32/f64` IEEE floats. Offsets are in bytes from the start of the structure. "Tick"
means one pulse at the file's PPQ. FL calls MIDI note 60 "C5".

---

## 1. Container

### 1.1 Chunks [C] [P-FMT]

A file is two chunks back to back, with nothing after the second (0 trailing bytes in all
255 files):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | ASCII `FLhd` |
| 4 | u32 | header body length, **6** in every file |
| 8 | i16 | file format: **0** = full project (every file) |
| 10 | u16 | number of channel-rack channels; equals the number of event-64 records [C] |
| 12 | u16 | PPQ (ticks per quarter note); **96** in every corpus file |
| 14 | 4 | ASCII `FLdt` |
| 18 | u32 | data length in bytes |
| 22 | … | the event stream, exactly `data length` bytes |

A reader should locate `FLdt` at `8 + header body length` rather than assume offset 14.

Other format values are documented for non-project files saved by FL (score 16, automation
24, channel state 32, plugin state 48, generator state 49, effect state 50, insert state 64)
[D] [P-PRJ]. PPQ 96 is the documented default; other PPQ values are possible and scale every
position and length [D] [P-PRJ].

### 1.2 Event framing [C] [P-FMT]

The stream is a flat sequence of events. Each starts with a one-byte **id**; the id alone
decides the payload size:

| Id range | Payload |
|---|---|
| 0–63 | 1 byte |
| 64–127 | u16 |
| 128–191 | u32 (sometimes holding an i32 or a colour) |
| 192–255 | a variable-length size, then that many bytes |

**Variable-length size [C]:** an unsigned integer written 7 bits per byte, least
significant group first; every byte except the last has its high bit (0x80) set. Examples
seen: `0x26` = 38; `0xC8 0x01` = 200; plugin states over 2 MB use 4-byte sizes. Every
corpus file parses to exactly the end of the data chunk with this rule.

There is no other framing: structure (which channel, pattern or insert an event belongs to)
comes only from event order (section 2).

### 1.3 Text [C]

- Event **199** (FL version) is ASCII, NUL-terminated, in every file (FL 12, 20 and 21).
- Every other text event in the corpus (FL 12.0.1, 20.7 and 21.2) is **UTF-16LE terminated
  by a two-byte NUL**. An empty string is just `00 00`.
- Strings inside the VST wrapper state (section 8.2) are 8-bit, not NUL-terminated, with an
  explicit length.
- Older FL versions (before roughly FL 11) stored text as 8-bit ANSI [I]; none are in the
  corpus. A reader can tell UTF-16 apart because the version event comes first (major ≥ 12
  means UTF-16 for everything seen).

---

## 2. File layout and context rules [C]

FL 20/21 files are written in this order:

1. **Project block:** 199, 159, 28, 37, 200, 156, 67, 9, 11, 80, 17, 18, 35, 23, 44, 10,
   194, 206, 207, 167, 202, 195, 237, then 231 (×1–3), 146, 216.
2. **Pattern note blocks:** for each pattern that has notes, `65 <id>` followed by 224 (notes)
   and/or 223 (controllers).
   Then 226 (×1 or ×3) and 227 (one per automation clip), in either order.
3. **Channel blocks**, one per channel, each opened by `64 <iid>`. **Pattern metadata
   blocks** (`65 <id>` followed by 193/150/157/158/164) are interleaved between channel
   blocks, in ascending pattern id order.
4. **Arrangement block(s):** `99 <index>`, 241 (name), 36, 233 (playlist items), then 500
   × (238 track data, 43), then 242, 100. A track name (239) was seen once, right after a
   238/43 pair [I: it names that track; c11 confirms].
5. **Mixer block:** 29, 39, 40, 31, 38, then 127 insert records (section 6), then 225 (all
   mixer parameters), 133, 243, 47.

**Context rule for readers:** event 64 opens a channel context; 65 opens a pattern context;
99 opens an arrangement; 236 marks the body of a mixer insert. Events that belong to these
objects apply to the most recently opened one. 64 and 65 can alternate, so the reader must
switch context on each.

The FL 12 file has no 99/241/242 (one implicit arrangement) and no 98 (section 6.4).

---

## 3. Project-level events

| Id | Type | Meaning | Tag |
|---|---|---|---|
| 199 | ASCII | FL version, `major.minor.patch.build`, e.g. `21.2.2.3914`; FL 12 wrote only three parts | C |
| 159 | u32 | FL build number (matches the last version part); absent in FL 12 | C |
| 156 | u32 | **Tempo in BPM × 1000** (corpus 76.000–151.000; 3 files have fractional BPM) | C (unit [D] [P-PRJ]) |
| 66 / 93 | u16 / u32 | old coarse/fine tempo; not written by FL 12+ | D [P-PRJ] |
| 17 | u8 | time signature numerator (beats per bar); 4 everywhere | I [P-ARR] |
| 18 | u8 | time signature beat / denominator; 4 everywhere | I [P-ARR] |
| 80 | u16 | main pitch in cents (signed; 0 everywhere) | D [P-PRJ] |
| 9 | u8 | loop active (song/pattern loop) | D [P-PRJ] |
| 10 | u8 | show project info on open | D [P-PRJ] |
| 23 | u8 | pan law (0 circular, 2 triangular); 0 everywhere | D [P-PRJ] |
| 28 | u8 | "licensed" flag | D [P-PRJ] |
| 67 | u16 | currently selected pattern (1–16 seen) | I |
| 146 | u32 | current channel filter group (0xFFFFFFFF = all) | D [P-PRJ] |
| 194 | UTF-16 | project title | C (empty in all files) |
| 195 | UTF-16 | comments | C (empty) |
| 197 | UTF-16 | URL | D |
| 198 | — | old RTF comments | D |
| 200 | UTF-16 | registered user name | C |
| 202 | UTF-16 | project data folder (absolute path or empty) | C |
| 206 | UTF-16 | genre | C (empty) |
| 207 | UTF-16 | author / artists | C (empty) |
| 237 | 16 bytes | f64 creation date as days since 1899-12-30 (corpus range: 2016–2026); f64 time spent working on the project, in days | C |
| 231 | UTF-16 | name of a channel filter group; one event per group, in order | I |
| 11, 30, 35–40, 44, 47, 99*, 100, 167 | u8/u16 | UI and option flags; constant or near-constant in corpus; not needed for import | unknown |
| 133 | u32 | one per file after the mixer parameters, values 126–958 | unknown |
| 216 | bytes | 0 or 12×n bytes: records `u32 0, u32 target id (section 9), u32 ?`; remote-control/controller links | I |
| 226 | 20 bytes | 1 or 3 per file, fixed contents; internal links | unknown |
| 227 | 20 bytes | one per automation clip: the clip's target (section 9) | C |

**PPQ** comes only from the header (offset 12).

---

## 4. Channels

### 4.1 Block contents [C]

Every channel block in FL 21 contains these events, in this order (some optional, marked ?):

`64 iid`, `21 type`, `201 internal plugin name`, `212 wrapper data`, `203? name`, `155 icon`,
`128 colour`, `41? (FL 21)`, `213? plugin state`, `0 enabled`, `209`, `138`, `139`, `89`, `97`,
`69`, `86`, `71`, `83`, `74`, `75`, `76`, `85`, `131`, `70`, `22 mixer insert`, `219 levels`,
`229`, `221 polyphony`, `215 parameters`, `132`, `144`, `145 filter group`, `32?`,
`228 ×2`, `218 ×5`, `143`, `20`, `196? sample path`, `234? automation points`.

| Id | Type | Meaning | Tag |
|---|---|---|---|
| 64 | u16 | channel iid; 0-based, consecutive in file order; referenced by notes and playlist items | C |
| 21 | u8 | **channel type**: 0 sampler, 2 generator plugin (native or VST), 3 layer, 4 audio clip, 5 automation clip | C for 0, 2, 4, 5; D for 3 [P-CH] |
| 201 | UTF-16 | plugin internal name: `Fruity Wrapper` for VST/VST3 hosts, the native plugin's factory name for native generators, empty for types 0, 4, 5 | C |
| 203 | UTF-16 | channel display name (present on almost every channel; when absent, FL shows a default) | C |
| 128 | u32 | channel colour, bytes R, G, B, 0 (default 0x00565148 = RGB 72, 81, 86) | C |
| 155 | u32 | channel icon id (0 = none) | I |
| 0 | u8 | channel enabled (1 on; 0 = muted in the rack) | I [P-CH] |
| 22 | u8 | **mixer insert the channel is routed to** (0 = master; 0–33 seen) | C |
| 145 | u8 | channel filter group index (into the 231 names) | I |
| 196 | UTF-16 | sample path (samplers and audio clips); may start with a placeholder such as `%FLStudioFactoryData%`, `%FLStudioData%` or `%USERPROFILE%`, otherwise a drive-letter absolute path | C |
| 132 | u32 | cut group: low u16 = cut, high u16 = cut by | I [P-CH] |
| 32 | u8 | channel locked | D [P-CH] |
| 20 | u8 | ping-pong loop | D [P-CH] |
| 143 | u32 | sampler flags (2, 3, 10, 258 seen) | unknown bits |
| 72 / 73 / 2 / 3 | | old volume/pan events; **not written by FL 12+** | D [P-CH] |
| 192 | | old channel name event; not written by FL 12+ | D [P-CH] |

### 4.2 Levels (219, 24 bytes) [C for layout, I for scale]

Six i32: pan, volume, pitch, then three more (0, 256, 0, 0 everywhere).

- **pan** 0–12800, **6400 = centre**, 0 = full left (0 and 4400 seen) [C + D P-CH];
- **volume** 0–12800, default **10000** (FL shows 78%); the curve is non-linear [D P-CH];
  corpus range 3250–12800;
- **pitch** in cents, 0 everywhere [I].

Event 229 (20 bytes, i32 × 5: 0, 12800, 0, 0, 0 everywhere) holds level adjustments [D P-CH].

### 4.3 Other fixed channel records (layout [C], meaning [D]/unknown)

| Id | Size | Contents seen |
|---|---|---|
| 209 | 20 | five i32 (0, 6400, 0, 4, 144): echo-delay settings [P-CH] |
| 221 | 9 | u32 max polyphony (0 = off), u32 slide time (500 default), u8 flags |
| 228 | 16 | two per channel: keyboard tracking (volume, then key), first i32 = 100/60 |
| 218 | 68 | five per channel: envelope/LFO for pan, volume, cutoff, resonance, pitch [P-CH] |
| 215 | 168 (FL 21), 158 (FL 20), 116 (FL 12) | sampler/arp parameters |
| 212 | 52 | 13 u32 of plugin window/host settings; u32[2] = 0xFFFFFFFF on channels without a plugin (types 0, 4, 5), 2 otherwise |
| 69–89, 97, 131, 138, 139 | fixed | filter, pre-amp, fades, swing etc. (constant in corpus) [P-CH] |

### 4.4 Plugin state of a channel

Generator channels (type 2) carry 213; others don't. See section 8.

### 4.5 Layers

Channel type 3 (layer) and its child list (event 94 "children" [D P-CH]) are **not in the
corpus**.

---

## 5. Patterns

### 5.1 Identity and metadata [C]

- `65 <id>`: pattern id, **1-based**. The same id opens both the note block (section 2,
  step 2) and the metadata block (step 3).
- 193 UTF-16: pattern name.
- 150 u32: pattern colour (R, G, B, 0; default 0x00565148).
- 164 u32: pattern length; 0 (= automatic) everywhere [P-PAT].
- 157, 158 u32: 0xFFFFFFFF everywhere; unknown [P-EV lists them as unknown].
- Patterns with no notes have only a metadata block; empty patterns with default name and
  colour may be omitted entirely [I].

### 5.2 Notes (224) [C for layout]

The payload is a whole number of **24-byte note records** (38 989 notes checked):

| Off | Type | Field | Corpus / meaning |
|---|---|---|---|
| 0 | u32 | position, ticks from pattern start | 0–56 832 |
| 4 | u16 | flags | 0x4000 everywhere; slide etc. untested [D P-PAT] |
| 6 | u16 | channel iid (matches an event-64 value in every record) | C |
| 8 | u32 | length in ticks (0 = step-sequencer note [D]) | 0–2304 |
| 12 | u16 | key: 0–131, 60 = C5 (MIDI 60) | 19–100 |
| 14 | u16 | note group (0 = none) | 0 |
| 16 | u8 | fine pitch: 120 = centre [P-PAT: 0–240 spans −1200…+1200 cents] | 120 only |
| 17 | u8 | unused | 0 |
| 18 | u8 | release, 0–128, default 64 | 0 or 64 |
| 19 | u8 | colour / MIDI channel 0–15; +128 marks notes imported from MIDI [D P-PAT] | 0, 1, 10, 129, … |
| 20 | u8 | pan 0–128, 64 = centre | 64 only |
| 21 | u8 | velocity 0–128, default 100 | 20–128 |
| 22 | u8 | mod X 0–255, default 128 | 128 only |
| 23 | u8 | mod Y 0–255, default 128 | 128 only |

Records are sorted by position (all 335 note events) [C]. A pattern can hold notes for several channels.

### 5.3 Controllers (223)

Seen twice (168 bytes, divisible into 12-byte records). PyFLP documents position, channel
and value fields [D P-PAT]; the layout is not confirmed.

---

## 6. Mixer

### 6.1 Insert records [C]

FL 20/21 always write **127 insert records**: index 0 = master, 1–125 = inserts, 126 =
"current" (a slot that isn't a real insert; it never routes anywhere). FL 12 writes 105.
[P-MIX documents 125 inserts + master + current since 12.9.]

Each record, in order:

1. `149 colour`? (u32 RGB0; only when set)
2. `42` (u8; FL 21 only; 0/1)
3. `95 icon`? (u16)
4. `204 name`? (UTF-16)
5. `236 flags` (12 bytes FL 20/21, 8 bytes FL 12) — the anchor: the n-th 236 is insert n−1
6. ten effect slots (section 6.4)
7. `235 routing` (127 bytes FL 20/21, 105 FL 12)
8. `165`, `166` (u32; FL 21 only)
9. `154 audio input`, `147 audio output` (i32; −1 = none)

So **colour, icon and name come before their insert's 236**. Proof: matching channel names
(203) against the names of the inserts they route to (22) gives 615 matches under this rule
and 24 under the opposite assumption.

### 6.2 Insert flags (236)

First u32 = 0, second u32 = flag bits, third u32 = 0. Values seen: 0x0C (master and
current), 0x44, 0x4C, 0x8C, 0x48, 0x104C, 0x2044. Bit meanings are unknown; PyFLP lists
flags for bypass-all, swap L/R, enabled, solo, locked, reverse polarity and dock position
(left/middle/right) [D P-INS]. Mute is expected among them.

### 6.3 Routing (235) [C]

One byte per insert (index 0…126): non-zero means "this insert sends to that insert".
Master and current route nowhere; 18 624 inserts route only to master; others have extra
targets. **The send level of each route has not been found** (see Unknowns).

### 6.4 Effect slots [C]

Inside an insert record, each of the 10 slots ends with `98 <slot index>` (u16, 0–9). A
slot that holds a plugin has the plugin events before its 98:
`201 internal name`, `212`, `203? user name`, `155 icon`, `128 colour`, `41?`, `213 state`.
An empty slot is just the 98.

FL 12 has no 98: occupied slots are written back to back without index, and the slot
position must come from the 225 parameter records (section 6.5) [I].

### 6.5 Mixer parameters (225) [C]

One event per file, a whole number of **12-byte records**:

| Off | Type | Field |
|---|---|---|
| 0 | u32 | 0 |
| 4 | u8 | parameter id |
| 5 | u8 | 0x1F |
| 6 | u16 | location: bit 13 set (0x2000), bits 6–12 = insert index, bits 0–5 = slot index |
| 8 | i32 | value |

Bytes 4–7 taken together form the **target id** that automation links use (section 9).

| Param id | Per | Meaning | Values |
|---|---|---|---|
| 0 | slot | slot enabled | 0/1 |
| 1 | slot | slot mix (dry/wet): 0–12800, 12800 = 100% wet | 1650–12800 seen. [P-SLOT gives −6400…6400; corpus disagrees] |
| 192 | insert | **volume**: 0 = silent, 12800 = 0 dB, 16000 = +5.6 dB max | 886–16000 [D curve, P-INS] |
| 193 | insert | **pan**: −6400 left … 0 … 6400 right | −2900–2100 |
| 194 | insert | stereo separation −64…64 | −56–39 |
| 208–210 | insert | EQ band gains (low, mid, high), −1800…1800 | 0 |
| 216–218 | insert | EQ band frequencies, raw 0–65536 → 10 Hz–16 kHz, non-linear; defaults 5777, 33145, 55825 | default only |
| 224–226 | insert | EQ band Q, default 17500 | default only |
| 164–168, 190 | insert | unknown; always 0 | |

One record per file has location bits 13–15 = 2 (0x4000), parameter 0; meaning unknown.

---

## 7. Playlist

### 7.1 Arrangements [C]

`99 <index>` (u16, 0-based) opens an arrangement; `241` UTF-16 is its name. Every FL 20/21
file in the corpus has exactly one. Multiple arrangements exist since FL 12.9.1 [D P-ARR].

### 7.2 Playlist items (233) [C]

One event per arrangement holding all its clips. Record size depends on the version:
**60 bytes in FL 21, 32 bytes in FL 20 and FL 12** (the first 32 bytes are the same).

| Off | Type | Field |
|---|---|---|
| 0 | u32 | position in ticks |
| 4 | u16 | 0x5000 (20480) everywhere: the pattern base |
| 6 | u16 | item index: **> 0x5000 → pattern clip, pattern id = index − 0x5000**; otherwise → channel clip (audio or automation), index = channel iid |
| 8 | u32 | length in ticks |
| 12 | u32 | track: **track number (0-based) = 499 − value** (FL 20/21) |
| 16 | u16 | 120 everywhere (unknown) |
| 18 | u16 | flags: 0x0040 always set; 0x2000, 0x4000, 0x8000 also seen. 0x2000 appears only on channel clips; mute is untested [I] |
| 20 | u16 | 0x6440 (unknown) |
| 22 | u16 | 0x8080 (unknown) |
| 24 | 4 | start offset |
| 28 | 4 | end offset |
| 32–59 | | FL 21 only: u32 (1–… for both kinds; unknown, possibly a clip group/serial), f32, f32, u32, f32, f32 = 1.0, u32 (0–3) |

Offsets differ by clip kind [C]:

- **pattern clips:** start/end offsets are **i32 ticks** within the pattern; 0xFFFFFFFF (−1)
  = not trimmed;
- **channel clips:** start/end offsets are **f32**; −1.0 = default; other values seen are
  0.0 and large fractional numbers (unit unknown, likely related to the sample, see Unknowns).

For channel clips the FL 21 tail sometimes has non-zero floats at 36/44 (unknown).

### 7.3 Tracks (238, 239) [C]

500 tracks per arrangement, each `238` (66 bytes FL 20/21, 47 bytes FL 12) followed by 43
(u8, FL 21), then 239 (UTF-16 name) when named [I, one sample]. The FL 12 file has 33 track
events and its 233 track values do not fit `499 − value`; its track numbering is unknown.

| Off | Type | Field |
|---|---|---|
| 0 | u32 | track iid, 1–500 |
| 4 | u32 | colour R, G, B, 0 (default RGB 72, 81, 86) |
| 8 | u32 | icon (0) |
| 12 | u8 | enabled (1 everywhere: track mute is presumably 0 here) [I] |
| 13 | f32 | height (1.0) |
| 17 | i32 | −1 or −16 (unknown) |
| 21–65 | | constant except rare values; PyFLP documents lock, motion, press, trigger sync, queued, tolerant, position sync and grouping fields [D P-TRK] |

---

## 8. Plugins

### 8.1 Identity [C]

A plugin (on a generator channel or in an effect slot) is identified by its **201 internal
name**:

- `Fruity Wrapper` = a third-party VST2 or VST3 plugin; identity is inside 213 (8.2);
- any other non-empty name = a **native FL plugin** with that factory name; 213 is its
  private state (opaque; we keep it as a placeholder blob);
- 203 (optional) is the name the user gave the instance.

### 8.2 VST wrapper state (213 when 201 = `Fruity Wrapper`) [C for framing]

`u32 version` (10 in FL 20/21; 8 seen too), then records until the payload ends:
`u32 record id`, `u64 length`, `length` bytes. Every wrapper payload in the corpus parses
exactly to its end.

| Record | Size | Meaning |
|---|---|---|
| 1 | 20 | wrapper options (MIDI ports etc.) [D P-VST] |
| 2 | 25 | wrapper flags [D P-VST] |
| 30 | 16 | unknown |
| 31 | 12–24 | unknown |
| 32 | 12–708 | unknown |
| 50 | 16 | first u32: plugin kind (0, 4, 7, 8 seen) |
| 51 | 4 | **VST2 unique id (FourCC)**, 4 ASCII bytes |
| 52 | 16 | **VST3 class id** (16 bytes) |
| 53 | any | **plugin state**: starts with u32 1 for VST3 (432 of 432), with i32 −9 (0xFFFFFFF7) for VST2 (340 of 340) |
| 54 | 4–20 | plugin name (8-bit text) |
| 55 | 34–76 | plugin binary path (8-bit text; `.dll` = VST2, `.vst3` = VST3) |
| 56 | 4–26 | vendor (8-bit text) |
| 57 | 4 | u32; appears with 51 (VST2 only) |

The inner layout of record 53 (where the plugin's own chunk begins) is unknown; an importer
can pass it through as an opaque blob. 129 wrappers hold only records 1 and 2 (no plugin
loaded or plugin missing) [I].

---

## 9. Automation

### 9.1 Automation clips [C]

An automation clip is a **channel of type 5**, placed on the playlist as a channel clip.
Its points are in event **234** inside the channel block:

- header, 21 bytes: u32 1, u32 64, u8 0, u32 4, u32 3, **u32 point count** (offset 17);
- `count` × 24-byte points: **f64 position delta** (quarter notes since the previous
  point; 0 for the first), **f64 value 0.0–1.0**, **f32 tension**, u32 (top byte varies:
  0x00, 0x01, 0x02, 0xFF seen; curve type?) [C layout; I for the last field];
- a 112-byte trailer (LFO and options; unknown).

Evidence for the delta reading: for untrimmed clips, the sum of the deltas × PPQ equals the
clip's playlist length exactly.

### 9.2 Automation targets (227) [C for layout, I for scope]

One 20-byte event per automation clip, in the project block: `u16 0, u16 channel iid of
the automation clip, u32 0, u32 target id, u32 8, u32 469`.

Target ids seen:

- `0x2xxx1Fpp`, the same encoding as bytes 4–7 of a 225 mixer record: **parameter `pp`,
  insert/slot in the high u16**. Seen (24 clips): mixer volume (param 192 = 0xC0) of inserts 0, 4 and 6;
- `0x40000000` and `0x40000005` (8 clips): a second space (perhaps channel or plugin parameters;
  unknown).

### 9.3 Other automation

Per-pattern automation (223 controllers) is rare (2 events); event automation inside
patterns and plugin parameter automation are not understood yet.

---

## 10. Version differences seen

| Feature | FL 12.0.1 (1 file) | FL 20.7.2 (3) | FL 21.2.2 (251) |
|---|---|---|---|
| Build event 159 | absent | present | present |
| Arrangement events 99/241/242/100 | absent | present | present |
| Inserts | 105 | 127 | 127 |
| 236 size / 235 size | 8 / 105 | 12 / 127 | 12 / 127 |
| Slot index event 98 | absent | present | present |
| 233 record size | 32 | 32 | 60 |
| 238 size | 47 | 66 | 66 |
| 215 size | 116 | 158 | 168 |
| FL 21 only | — | — | 40–44, 47, 165–167, 242, 243 |

FL 21 never writes an event FL 20 lacks except those listed, and writes none that FL 20 has
but 21 dropped.

---

## 11. Unknowns (with evidence so far)

1. **Route send levels.** 235 gives the targets; no 225 parameter carries a level, and no
   other event varies with routing. Needs controlled file 6.
2. **Insert flag bits (236)** and **mute/solo** for inserts: values in 6.2.
3. **Playlist flags (233 off 18)** and clip mute; FL 21 tail fields; channel-clip offset
   unit (values like 66.96 and 49 043.48 are not ticks of the clip).
4. **Track mute/solo** (238).
5. **Time signature:** 17/18 are always 4; whether 18 is a denominator or steps per beat,
   and per-arrangement time-signature markers.
6. **Volume curves:** raw→dB mapping for channel volume (0–12800) and insert volume
   (0–16000).
7. **Automation:** the 112-byte trailer, the last field of each point, target ids of the
   0x4000xxxx space, and how plugin parameters are addressed.
8. **Layer channels** (type 3) and their children.
9. **Event 133**, 226, 242, 243, 41, 42, 43, 165, 166, 167: unknown, probably UI state.
10. **Native plugin states (213):** opaque by design.
11. **Note flags** (slide, porta) and non-default fine pitch/pan/mod X/mod Y — only defaults
    seen.
12. **PPQ other than 96** — none in corpus.

---

## 12. Controlled test files (request to the owner)

Save each as a new FL 21 project from **File > New** (the empty template), with the name in
bold, into a folder you will hand over (for example `Documents\flp-fixtures`). They become
allowed fixtures because you make them. Each takes 1–3 minutes. Keep everything else at its
default.

1. **`c01-tempo`**: an empty project. Set the tempo (top toolbar) to 123.456 BPM: right-click
   the tempo, choose a fine mode or type 123.456. Save. *(Tempo unit and precision.)*
2. **`c02-timesig`**: an empty project. **Options > Project general settings**, set
   **Time signature** to 7/8. Save. *(Events 17/18.)*
3. **`c03-note`**: in pattern 1, open the piano roll of the first channel and draw one note
   **C5 at bar 2, beat 1**, exactly one beat long. Open its properties (double-click the
   note) and set **velocity 50%**, **pan 100% left**, **fine pitch +50 cents**, **release
   25%**, **mod X 25%**, **mod Y 75%**. Save. *(Note field scales.)*
4. **`c04-ppq`**: like c03 but first set **Options > Project general settings > Timebase
   (PPQ)** to 960. Save. *(PPQ scaling.)*
5. **`c05-steps`**: in the channel rack, click steps 1 and 5 of the first channel; draw a
   second note in the piano roll with the **slide** flag on and another with **portamento**.
   Save. *(Step notes, note flags.)*
6. **`c06-routing`**: route the first channel to insert 3. In the mixer: insert 1 volume
   **−6.0 dB**, pan **100% left**; **mute** insert 2; **solo** insert 4; name insert 3
   **Bus** and colour it red; on insert 1 turn off the route to master and add a route to
   insert 3 with a **send level of 50%**. Save. *(Routes, send levels, insert flags.)*
7. **`c07-slots`**: on insert 1, put **Fruity Parametric EQ 2** in slot 1 and **disable** the
   slot; put any **VST2** plugin in slot 3 and any **VST3** plugin in slot 5; set slot 3's
   mix to 50%. Save. *(Slot enable/mix, VST identity.)*
8. **`c08-playlist`**: draw a 1-bar pattern (one note), place it at bar 1 on track 1 and at
   bar 5 on track 3; **mute** the second clip (click its corner). Make the first clip start
   one beat later by dragging its left edge. Save. *(Clip positions, trim offsets, clip
   mute.)*
9. **`c09-audio`**: drag any short WAV into the playlist at bar 3, track 2. Cut its start so
   it begins **half a beat** in, and shorten it to one bar. Note the WAV's sample rate and
   length. Save. *(Audio clip offsets and their unit.)*
10. **`c10-automation`**: right-click the **master volume** fader > **Create automation
    clip**. In the clip, make three points: bar 1 at 100%, bar 2 at 0%, bar 3 at 50%; set
    the tension of the second segment to about +50%. Then create a second clip for
    **insert 2 pan**, and a third for the **cutoff knob of a 3x Osc** channel. Save.
    *(Points, tension, targets.)*
11. **`c11-tracks`**: name playlist track 1 **Alpha**, **mute** track 2, colour track 3
    green, make track 4 taller. Save. *(Track data fields.)*
12. **`c12-layer`**: add a **Layer** channel (**Add > Layer**) and make two 3x Osc channels
    its children (select them, then in the Layer press **Set children**). Save. *(Type 3,
    children.)*
13. **`c13-arrangements`**: put a pattern clip on track 1 of arrangement 1; create a second
    arrangement named **Alt** (arrangement picker > **New**) and put the clip at bar 2 of
    track 2. Save. *(Multiple arrangements.)*
14. **`c14-info`**: **Options > Project info**: title **T1**, author **A1**, genre **G1**,
    comments **C1**. Set the main pitch knob to +3 semitones and the main volume to 50%.
    Save. *(Metadata and master knobs.)*
15. **`c15-channel-levels`**: three default channels: set channel 1 volume to **100%**,
    channel 2 to **0%**, channel 3 pan to **100% right**; mute channel 3 (its green light).
    Save. *(Channel level scales, enable flag.)*

---

## 13. Checking this spec: `tools/flp-probe`

- `cargo run -p flp-probe -- dump <file.flp> [--preview N]` prints every event: file offset,
  id, and the value (fixed events), the decoded string (text events) or the length and first
  N bytes in hex (binary events).
- `cargo run -p flp-probe -- stats <file-or-folder>...` prints aggregate numbers only: files
  per FL major version, PPQ and format values, and for each event id its size class, how many
  files and events have it, the value range (fixed) or length range (variable), and how many
  payloads look like UTF-16 or ASCII text.

The probe knows only the container (section 1). Everything else here is for the importer to
implement.
