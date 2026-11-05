# Logic EXS24 / Sampler instrument (`.exs`): format specification

**Status:** M3/T1, first edition (2026-11-05). Written for the EXS importer (M3/T4), which
is implemented from this document alone.

**Clean-room statement.** Apple has never published the `.exs` format. Everything here
comes from public third-party sources: one community format note, one public gist, and
open-source readers (ConvertWithMoss, LGPL-3.0; renoise-exs24), which were **consulted for
facts only** (field offsets, ids, flag bits). No code was copied, translated or
restructured from them; the layout is described in our own words and order. Logic and its
EXS/Sampler plug-ins were **not** disassembled or decompiled. No `.exs` file has been
examined yet: there is no owner library folder and no generated corpus.

**Evidence tags:**
- **[D]** documented by public third-party sources, not seen on files. "×2" or "×3" means two
  or three independent sources agree;
- **[D?]** documented, but sources disagree (the disagreement is stated);
- **[I]** inferred;
- **[C]** confirmed on files: **nothing yet**;
- **[PH]** a Patternhaus decision.
There is no **[S]** (no official spec).

**Cited sources:**
- [CWM-DOC] ConvertWithMoss format documentation, "Logic EXS24" section:
  https://github.com/git-moss/ConvertWithMoss/blob/main/documentation/README-FORMATS.md
- [CWM-SRC] ConvertWithMoss source, package `format/exs` (`EXS24Block`, `EXS24Instrument`,
  `EXS24Zone`, `EXS24Group`, `EXS24Sample`, `EXS24Parameters`, `EXS24Detector`):
  https://github.com/git-moss/ConvertWithMoss/tree/main/src/main/java/de/mossgrabers/convertwithmoss/format/exs
  (read for facts only).
- [CWM-409] ConvertWithMoss PR #409, "read the big-endian variant and instruments with
  consolidated samples": https://github.com/git-moss/ConvertWithMoss/pull/409
- [TONV] "EXS24 format spec" in tonverk-elmulti-converter (a community note that credits
  ConvertWithMoss, so not independent of it):
  https://github.com/asatamax/tonverk-elmulti-converter/blob/main/docs/EXS24_FORMAT_SPEC.md
- [RNS] renoise-exs24 tool (Lua), read for facts only:
  https://github.com/matt-allan/renoise-exs24/blob/master/exs24.lua
- [GIST] jpcima, EXS format notes: https://gist.github.com/jpcima/09328280f6832b61892e416d261073e5
- [APPLE] Apple's Logic/MainStage EXS24 mkII help (concepts only: zones, groups,
  parameters; no binary layout):
  https://help.apple.com/logicpro/mac/9.1.6/en/logicpro/instruments/chapter_12_section_4.html

Independence: [CWM-SRC] and [TONV] are one lineage; [RNS] and [GIST] are separate. A fact
marked **[D ×2]** or better has at least one source outside the ConvertWithMoss lineage.

Conventions: offsets in bytes. "Content offset" = from the first byte after a block's
84-byte header. Integers are in the **file's byte order** (section 1.2).

---

## 1. Container

### 1.1 Blocks [D ×3: CWM-SRC, RNS, GIST]

A file is a flat sequence of blocks, each an 84-byte header followed by `size` bytes of
content. The next block starts at `offset + 84 + size`. There is no file-level header
before the first block and nothing after the last [D ×2: RNS, CWM-409 "chains exactly to
the end of the file"].

Block header:

| Off | Size | Field | Evidence |
|---|---|---|---|
| 0 | 4 | signature: in a little-endian file the bytes are `01 01 00 TT`, i.e. the u32 `0xTT000101` (TT = type byte with flags) | D ×3 |
| 4 | u32 | content size (excludes the 84-byte header) | D ×3 |
| 8 | u32 | object id / index | D ×2 (CWM-SRC, RNS) |
| 12 | u32 | flags; values 2, 3, 64, 0x80000002 observed; meaning unknown | D (CWM-SRC) |
| 16 | 4 | ASCII magic: `TBOS`, `JBOS`, `SOBT` or `SOBJ` (1.2) | D ×2 (CWM-SRC, RNS) |
| 20 | 64 | object name, NUL-terminated (ASCII; treat as UTF-8 with fallback [PH]) | D ×3 |

Per-byte reading of offset 0–3 [D, CWM-SRC]: byte 0 = byte-order flag (0 = big-endian,
else little-endian); byte 1 = 1 and byte 2 = 0 (format version; a reader tolerates one of
them differing); byte 3 = type and flags.

**[D?] big-endian signature bytes:** [TONV] lists big-endian signatures that read as
`0x01010000` (header) and `0x01010001` (zone), i.e. bytes `00 00 01 01` / `01 00 01 01`,
which do not match the per-byte reading above (`00 01 00 TT`). Confirming this needs a
big-endian file (see section 9). **[PH]:** decide byte order from the **magic** (1.2), and
take the type from byte 3 when byte 0 is non-zero, else from whichever of bytes 0 and 3 has
a valid type nibble; report the variant.

### 1.2 Byte order [D ×2]

| Magic at 16 | Byte order |
|---|---|
| `TBOS` (standard), `JBOS` (variant) | little-endian |
| `SOBT` (standard), `SOBJ` (variant) | big-endian |

Any other magic: not an EXS file (refuse). Every block carries the magic; a block whose
magic disagrees with the first block's byte order is an error [D, CWM-SRC: "read fails"].
Older (PowerPC-era) instruments are big-endian [I, CWM-409 calls it a legacy variant].
The difference between `T` and `J` variants is unknown [D?].

### 1.3 Type byte [D ×2]

Type = `byte3 & 0x0F` (for little-endian; see 1.1 for big-endian):

| Type | Block | Content size seen |
|---|---|---|
| 0x00 | instrument (file header) | 40 [D, CWM-409] |
| 0x01 | zone | ≥ 96; 104 in the files of CWM-409 [D ×2] |
| 0x02 | group | 76 or 92 (writer emits 92); 96 in CWM-409 [D] |
| 0x03 | sample | 336 or 592 [D ×2: CWM-SRC, RNS] |
| 0x04 | parameters | 304 [D, CWM-409]; can be longer (section 6) |
| 0x05 | tagged float records, first tag `GAMETSPP`; not part of the mapping; skip [D, CWM-409] |
| 0x07, 0x08 | unknown; type 8 seen as 16 zero blocks of size 4 after some headers [D, CWM-SRC] |
| 0x0A | sampler layout, a binary property list [D, CWM-SRC] |
| 0x0B | a macOS property list with sample-file information [D, CWM-SRC] |

Flags in the high nibble: **0x40** marks a newer variant (signatures `0x4T000101`) [D ×2:
CWM-SRC, TONV]; **0x80** marks the big-endian variant of CWM-409, in which **bit 15 of the
size field is also set** and must be cleared (`size & 0x7FFF`) [D, CWM-409, CWM-SRC]. [RNS]
handles the same thing differently: if the first block's size exceeds 0x8000, subtract 0x8000
from every block size [D]. **[PH]:** if the 0x80 flag is set or the size exceeds the bytes
left in the file, clear bit 15 and re-check; report "size flag".

Unknown types are skipped by size [PH].

### 1.4 Block order [D ×2]

Instrument block first, then zones, groups, samples, then parameters (and optional extra
blocks) [D: TONV; RNS walks blocks in any order]. **[PH]:** accept any order after the
instrument block; index zones, groups and samples by their order of appearance **within
each type** (0-based), which is what zone references use (section 3).

---

## 2. Instrument block (type 0) [D, CWM-SRC only]

Content (u32 each):

| Off | Field |
|---|---|
| 0 | always 0 |
| 4 | number of zone blocks |
| 8 | number of group blocks |
| 12 | number of sample blocks |
| 16 | number of parameter blocks |
| 20–31 | unknown |
| 32 | unknown; when set, 16 blocks of type 8 follow |
| 36 | unknown |

The header name (offset 20) is the instrument name. **[PH]:** use the counts only as a
cross-check against the blocks found; report mismatches.

---

## 3. Zone block (type 1)

Content offsets [D ×3 for 0–33 and 88–95: CWM-SRC, RNS, GIST (GIST gives the same
offsets +84); D ×2 for the rest]:

| Off | Type | Field | Notes |
|---|---|---|---|
| 0 | u8 | zone options | bits below |
| 1 | u8 | root key 0–127 | |
| 2 | i8 | fine tune, cents | range ±50 in the UI [I] |
| 3 | i8 | pan | CWM-SRC converts with ÷50 to −1…1 (so ±50 is full); TONV says −64…63 [D?] |
| 4 | i8 | volume adjust, dB | documented range −96…+12 [D, CWM-SRC] |
| 5 | u8 | volume scale (key scaling) | unit unknown |
| 6 | u8 | key low | |
| 7 | u8 | key high | |
| 8 | u8 | unused | |
| 9 | u8 | velocity low | used only if option bit 3 is set |
| 10 | u8 | velocity high | |
| 11 | u8 | unused | |
| 12 | u32 | sample start, frames | |
| 16 | u32 | sample end, frames | |
| 20 | u32 | loop start, frames | |
| 24 | u32 | loop end, frames: **exclusive** (a reader subtracts 1 for an inclusive end) [D, CWM-SRC] | |
| 28 | u32 | loop crossfade, **milliseconds** [D, CWM-SRC; TONV] | |
| 32 | u8 | loop tune, cents (÷100 → semitones) [D, CWM-SRC] | |
| 33 | u8 | loop options | bits below |
| 34 | u8 | loop direction: 0 forward, 1 backward, 2 ping-pong | D ×2 (CWM-SRC, TONV) |
| 35–76 | | unused | |
| 77 | u8 | flex options | skip |
| 78 | u8 | flex speed | skip |
| 79 | u8 | tail tune | skip |
| 80 | i8 | coarse tune, semitones | D ×2 (CWM-SRC, RNS) |
| 81 | | unused | |
| 82 | u8 | output assignment; valid only if option bit 6 is set | |
| 83–87 | | unused | |
| 88 | i32 | group index, 0-based; −1 = no group | D ×3 |
| 92 | i32 | sample index, 0-based; −1 = no sample (zone is skipped) | D ×2 |
| 96–103 | | unknown (present when content ≥ 104) | |
| 104 | u32 | sample fade-out, only when the content is longer than 104 bytes | D (RNS) |
| 108 | u32 | sample offset, only when the content is longer than 108 bytes | D (RNS) |

Zone options (offset 0) [D ×2 (CWM-SRC, RNS); D? vs GIST]:
- bit 0: one-shot (ignore note-off);
- bit 1: **pitch tracking off** when set (inverted flag);
- bit 2: reverse;
- bit 3: velocity range on (otherwise the zone covers 0–127);
- bit 6: output assignment valid.
[GIST] instead gives bit 0 = pitch tracking, bit 1 = one-shot. **[PH]:** follow the
two-source reading; fixture-free confirmation needs an owner file with a one-shot zone
(section 9).

Loop options (offset 33) [D ×2 (CWM-SRC, RNS) for bits 0–1; D for bit 2]:
- bit 0: loop on (loops apply only when set);
- bit 1: equal-power crossfade (else linear);
- bit 2: play to end on release (loop while held, then play out).

**Zone references [D, CWM-SRC]:** if **no** zone has a sample index ≥ 0, each zone's
**object id** (header offset 8) is used as its sample index instead. Zone ids ≤ 0 mean
"use the zone's position". **[PH]:** follow this fallback and report it.

---

## 4. Group block (type 2) [D, CWM-SRC; offsets 0–75 also TONV; 2, 73, 74, 80 also GIST]

| Off | Type | Field | Notes |
|---|---|---|---|
| 0 | i8 | volume, dB (−96…+24) | added to each zone's volume |
| 1 | i8 | pan | added to zone pan (same scale as zone) |
| 2 | u8 | polyphony; 0 = maximum | |
| 3 | u8 | options: 0x10 mute, 0x40 release-trigger decay, 0x80 fixed sample select | |
| 4 | u8 | exclusive group id (choke) | |
| 5 | u8 | velocity low limit (default 0) | clips zones; a zone fully outside is dropped |
| 6 | u8 | velocity high limit (default 127) | |
| 7 | u8 | sample-select random offset; > 0 = zones play randomly | |
| 8–15 | | unused | |
| 16 | u16 | release trigger time | unit unknown |
| 18–31 | | unused | |
| 32 | u8 | velocity-range crossfade, stored +128 (value = byte − 128) | |
| 33 | u8 | velocity crossfade type | values unknown |
| 34 | u8 | key-range crossfade type | |
| 35 | u8 | key-range crossfade, stored +128 | |
| 36–37 | | unused | |
| 38 | u8 | enable-by-tempo low (default 80) | |
| 39 | u8 | enable-by-tempo high (default 140) | |
| 40 | | unused | |
| 41 | u8 | filter cutoff offset | sign unknown |
| 42 | | unused | |
| 43 | u8 | resonance offset | |
| 44–55 | | unused | |
| 56, 60, 64, 68 | u32 ×4 | env 1 attack, decay, sustain, release offsets | units unknown |
| 72 | | unused | |
| 73 | u8 | release trigger: non-zero = the group plays on note-off | D ×3 |
| 74 | u8 | output | |
| 75 | u8 | enable-by note value | |
| *76–79* | | *(extended part, present when size is 92)* unused | |
| 80 | i32 | round-robin position; −1 = none (default) | D ×3 |
| 84 | u8 | enable-by type: 0 none, 1 note (keyswitch), 2 round robin, 3 control (CC), 4 pitch bend, 5 MIDI channel, 6 articulation id, 7 tempo | |
| 85 | u8 | enable-by control: CC number | |
| 86 | u8 | enable-by control low (default 0) | |
| 87 | u8 | enable-by control high (default 127) | |
| 88 | u8 | key range start (default 0); group key limit | |
| 89 | u8 | key range end (default 127) | |
| 90 | u8 | enable-by MIDI channel | |
| 91 | u8 | enable-by articulation id | |

Rules [D, CWM-SRC] / [PH]:
- **Round robin:** a group whose enable-by type is 2 belongs to a round-robin cycle; its
  **sequence position = stored position + 2** [D, CWM-SRC; the +2 is unexplained]. **[PH]:**
  collect groups with enable-by 2, sort by stored position, and number them 1…n in that
  order; cycle length = n. Confirm on files (section 9).
- **Keyswitch (enable by note):** the group plays only after key *enable-by note value*
  (offset 75) was the last keyswitch pressed [I from the field names and [APPLE]'s
  "Enable by Note" concept]. **[PH]:** map to native keyswitches, `sw_last`-style.
- **Enable by control:** the group plays only while CC *n* is within low…high [I]. Map to
  a CC condition.
- **Articulation id:** maps to the articulation-set mechanism of Logic; keep as data, map
  to a keyswitch-like articulation switch with id = value [PH].
- Tempo, bend and channel enable: keep as data, skip, report.
- Group velocity/key limits of 0 mean "no limit" [D, CWM-SRC].
- Mute bit: the group is silent; import it as a muted group [PH].

---

## 5. Sample block (type 3) [D ×2: CWM-SRC, RNS; TONV agrees]

| Off | Type | Field |
|---|---|---|
| 0 | u32 | wave-data start (offset of audio within the sample file) |
| 4 | u32 | length, frames |
| 8 | u32 | sample rate, Hz |
| 12 | u32 | bit depth (16, 24, 32) ([RNS] reads one byte) |
| 16 | u32 | channels |
| 20 | u32 | a second channel count (meaning unknown) |
| 24 | | unused |
| 28 | 4 | file type FourCC, e.g. `WAVE`, `AIFF` (byte order of the FourCC unknown [D?]) |
| 32 | u32 | file size |
| 36 | u32 | compressed flag (non-zero = compressed, e.g. CAF/AAC) |
| 40–79 | | unused |
| 80 | 256 | **path** string, NUL-terminated |
| 336 | 256 | **file name** string, NUL-terminated (only in the 592-byte form) |

- In the 336-byte form the file name is the block header's name [D ×2].
- [CWM-DOC] says the format stores **absolute paths** only. Whether offset 80 holds the
  folder or the full path is not settled [D?]: [TONV] calls it the full path; the presence
  of a separate name field suggests a folder [I].
- The header's object name is the sample's display name.

**Sample lookup [PH], following the behaviour described in [CWM-DOC]:**
1. If `path` (or `path` + `/` + name) names an existing file, use it (macOS paths will rarely
   exist on Windows).
2. Look for `name` in the `.exs` file's folder.
3. Search recursively for `name` (case-insensitive) under the `.exs` folder, then under its
   parent folders up to a configurable depth (default 2 levels up).
4. Report every sample not found; zones that use it are skipped.

**Consolidated samples [D, CWM-409, CWM-DOC]:** Logic can save an instrument whose zones all
use **one** long audio file (in one case a 17-minute AAC-compressed CAF), each zone
playing its own start…end section. Import keeps this as one shared sample with per-zone
ranges [PH]. CAF/AAC decoding is outside `ph-formats` today: report "compressed sample
(CAF/AAC) not supported" and skip such samples unless T4 adds a decoder [PH].

---

## 6. Parameters block (type 4) [D, CWM-SRC; size 304 matches D, CWM-409]

Layout:
1. **Section 1:** `u32 count` (normally 100), then `count` × `u8` parameter ids (0 = empty
   slot), then `count` × `i16` values; value *i* belongs to id *i*. With count 100 this is
   4 + 100 + 200 = **304 bytes** [D ×2: CWM-SRC layout, CWM-409 size].
2. **Section 2** (only if bytes remain): `u32 count` (normally 200), then `count` records
   of `u16 id`, `u16 value` (unsigned); id 0 = empty. Ignore the section if the count does
   not fit the remaining bytes.

Parameter ids and conversions [D, CWM-SRC; conversions are that reader's mapping, not a
spec, so treat the formulas as **approximations**]:

| Id | Meaning | Conversion used by CWM-SRC | M3 |
|---|---|---|---|
| 0x07 | master volume | dB [I] | must |
| 0x160 | master pan | | must |
| 0x08 | volume key scale | ÷1000 → amp key tracking −1…1 | approximate |
| 0x03 / 0x04 | pitch bend up / down | semitones ×100 → cents; down = −1 means "same as up"; default ±200 cents | must |
| 0x05 | polyphony (max 99) | voices | must |
| 0x0A | mono / legato | > 0 = legato | must |
| 0x116, 0x11A | MIDI mono mode, its pitch range | | skip |
| 0x2D | transpose | semitones | must |
| 0x0E / 0x0F | coarse / fine tune | semitones / cents | must |
| 0x14 | glide | not converted | approximate |
| 0x48 / 0x49 | portamento down / up | | skip |
| 0xA6 | coarse tune remote | | skip |
| 0x2C | filter 1 on | | must |
| 0xF3 | filter 1 type: 0–3 low-pass 4/3/2/1-pole; 4 high-pass 2-pole; 5 band-pass 2-pole | | must |
| 0x1E | filter 1 cutoff | ÷1000 → 0…1 of the filter's range; default 1000 | must (approximate curve) |
| 0x1D | filter 1 resonance | ÷1000 → 0…1 | must |
| 0x4B | filter 1 drive | | skip |
| 0x2E | filter 1 key track | ÷1000 | must |
| 0xAA | filter 1 "fat" | | skip |
| 0x174, 0x186, 0x177, 0x178, 0x179, 0x173, 0x17A | filter 2 on, type, cutoff, resonance, drive; serial/parallel; blend | | approximate (second filter in series) |
| 0x3C, 0x3D, 0x3E | LFO 1 fade, rate, wave | | approximate |
| 0x14C–0x150, 0x187 | LFO 1 key trigger, mono/poly, phase, polarity, tempo sync, fade direction | | skip |
| 0x3F, 0x40, 0x151–0x156, 0x188 | LFO 2 (same set) | | approximate |
| 0xA7, 0x157–0x15D, 0x189 | LFO 3 (same set) | | skip |
| 0x6B | env 1 (amp) type: 0 AD, 1 AR, 2 ADSR, 3 AHDSR, 4 DADSR, 5 DAHDSR | | must |
| 0x52 / 0x53 | env 1 attack at high / low velocity | time = 10 × (p/127)⁴ s; velocity tracking = (low − high)/127 | must |
| 0x58, 0x54, 0x51, 0x55 | env 1 hold, decay, sustain, release | times as above; sustain = p/127 | must |
| 0x16D | env 1 delay | time as above | must |
| 0x195 | env 1 attack curve | values ≥ 0xFF00 wrap negative; ÷99 → −1…1 | approximate |
| 0x5A | env 1 velocity sensitivity | ÷(−60) → 0…1; default 1 | must |
| 0x59 | env 1 volume high | | approximate |
| 0x16A, 0x17D, 0x16C, 0x192, 0x4C, 0x4D, 0x38, 0x4E, 0x4F, 0x50, 0x5B | env 2 (filter/mod) type, vel sens, delay, attack curve, attack hi/lo vel, hold, decay, sustain, release, time curve | as env 1 | must (filter envelope) |
| 0x183 | amp velocity curve | | approximate |
| 0x5F | velocity offset | | approximate |
| 0xA4 | random velocity | | skip |
| 0xA3 | random sample select (global): zones play randomly | | must |
| 0x62 | random pitch | | approximate |
| 0x61, 0xA5 | crossfade amount, type | | approximate |
| 0xAB | unison | | skip |
| 0xAC | hold via control | | skip |
| 0xAD…0xFD, 0x19B…0x1A2 | 11 modulation-matrix slots, each: destination, source, via, amount low, amount high, source invert, via invert, bypass (ids per slot in [CWM-SRC]) | source/destination enumerations not documented | keep as data; map only "key → cutoff" (filter key tracking fallback), "env 2 → cutoff" (depth = amount low ÷ 1000) and "velocity → cutoff" [D, CWM-SRC] |

Modulation slot ids (destination, source, via, amount low, amount high, source invert,
via invert, bypass) [D, CWM-SRC]: slot 1 = 0xAD, 0xAE, 0xAF, 0xB0, 0xB1, 0xE9, 0xB2, 0xF4;
slots 2–10 continue in steps of 6 for the first five fields (0xB3…, 0xB9…, … 0xE3…), with
invert ids 0xEA…0xF2 and bypass ids 0xF5…0xFD; slot 11 = 0x19B…0x1A2.

Unknown ids are kept as data and reported by number [PH].

---

## 7. Mapping to Haus Sampler [PH unless marked]

- One `.exs` = one instrument; group blocks → native groups (name from the block header);
  zones with group −1 go to an implicit "Ungrouped" group.
- Zone pitch: semitones = coarse + fine/100 (+ global transpose, coarse, fine); pitch
  tracking off → fixed pitch [D, CWM-SRC].
- Zone gain = zone volume dB + group volume dB (+ master volume); pan = zone + group pan,
  clamped [D, CWM-SRC].
- Loops: only when loop bit 0 is set; end exclusive (subtract 1 for our inclusive model);
  crossfade ms → frames at the sample's rate; direction 1 → backward, 2 → ping-pong;
  "play to end on release" → loop-while-held [D, CWM-SRC].
- Release-trigger groups → release triggers; release-trigger decay (option 0x40) →
  release decay with a default of 3 dB/s, reported as approximated.
- Exclusive group → choke group.
- Envelope and filter parameters from the parameters block apply to all zones.

---

## 8. Encryption and protection

No public source mentions encrypted `.exs` files [D: none of the sources above]. An
instrument may reference sample files that are themselves protected or in a proprietary
container; such samples simply fail to decode and are reported. The importer never
attempts anything beyond reading the file as described.

---

## 9. Unknowns, and facts that need real files

Facts that should be confirmed on `.exs` files from the owner (or on files the owner saves
in Logic for this purpose) before T4 relies on them:

1. Big-endian signature bytes (1.1) and the `T`/`J` magic variants.
2. Zone option bits 0/1 (one-shot vs pitch), disputed between sources.
3. Pan scale (±50 vs ±64).
4. Loop end exclusive, crossfade in ms.
5. Round-robin position encoding (+2 offset) and how Logic orders the cycle.
6. Whether the sample path is a folder or a full path.
7. Group crossfade types and amounts; key-range crossfade behaviour.
8. Units of group envelope and filter offsets.
9. Parameter value scales (master volume units, cutoff curve, envelope time curve).
10. Type 0x0A / 0x0B property-list contents (newer Logic "Sampler" files).
11. Modulation-matrix source/destination enumerations.

**Controlled files the owner could save** (Logic Sampler, new instrument each, any short
WAV): (a) one zone C3 root, keys C2–C4, no options; (b) the same with one-shot on; (c) pitch
off; (d) pan hard left, volume −6 dB, fine +25, coarse +2; (e) a forward loop with 10 ms
crossfade, equal power; (f) two groups as a round robin of 3; (g) two groups selected by
note (keyswitch C0/D0); (h) a release-trigger group; (i) filter LP 2-pole at a noted
cutoff/resonance; (j) amp envelope with noted A/D/S/R values. Each isolates one fact above.

---

## 10. Fixture plan (generated, never copied)

A small EXS **writer** in test code (both byte orders) builds every fixture from the layout
above. Because the layout is unconfirmed, these fixtures test **our reader's agreement with
this document**, not compatibility with Logic; the owner files in section 9 do that.

| Id | Content | Exercises |
|---|---|---|
| F-EXS-01 minimal LE | instrument, 1 zone, 1 group, 1 sample (592 form), parameters | block chain, LE, sample lookup in the same folder |
| F-EXS-02 minimal BE | the same, big-endian with the 0x80 flag and size bit 15 set | byte order, size flag |
| F-EXS-03 short forms | 336-byte sample (name from header), 76-byte group, 96-byte zone | size variants |
| F-EXS-04 mapping | 12 zones: key/vel ranges with and without option bit 3, tunes, pan, volume, pitch off, reverse, one-shot | zone fields |
| F-EXS-05 loops | each direction, crossfade both types, play-to-end, loop off with loop points present | loops |
| F-EXS-06 groups | volume/pan/exclusive, velocity and key limits clipping zones, release trigger, mute | group fields |
| F-EXS-07 enable-by | round robin of 3 groups, keyswitch groups, CC-enabled groups | round robin, keyswitch |
| F-EXS-08 params | section 1 + section 2, envelopes, filter types, pitch bend −1, unknown ids | parameters |
| F-EXS-09 lookup | absolute macOS path that does not exist, sample in a sibling folder, case-mismatch | lookup rules |
| F-EXS-10 malformed | wrong magic, mixed magics, size past end, unknown block types 5/7/8/0x0A, zone sample index out of range, all indices −1 (id fallback) | refusal and fallbacks (fuzz seeds) |
