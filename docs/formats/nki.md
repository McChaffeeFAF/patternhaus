# Native Instruments Kontakt files (`.nki`, `.nkm`, `.nkb`, `.nkx`/`.nkc`, `.ncw`): format specification

**Status:** M3/T1, first edition (2026-11-05). Written for the Kontakt importer (M3/T6),
which is implemented from this document alone. **Unencrypted content only.**

**Clean-room statement.** Native Instruments publishes no file-format documentation. Every
fact here comes from public third-party sources: the ConvertWithMoss documentation and
source code (LGPL-3.0), **consulted for facts only** (magic numbers, field order, sizes,
ids), the documentation of the `ncw` Rust crate, NI's own public Kontakt manual (file
extension names only), and public forum posts. No code was copied, translated or
restructured; the layout is described in our own words and order. Kontakt was **not**
disassembled, decompiled, debugged or run for this work. No Kontakt file has been examined
yet (no owner folder).

**Boundary (DESIGN §8, M3.md).** This document specifies how to **detect** encrypted or
licence-locked content so that the importer can **refuse** it. It contains nothing about
the content of encrypted data, keys, or any way around protection. Section 9 is the
complete list of detection rules. Implementers must not add to it by experimentation on
protected files.

**Legal note for the owner (not a format fact).** In April 2024 Native Instruments had the
public GitHub repository `monomadic/ni-file` ("a reverse engineering effort for the Native
Instruments file formats") taken down with a DMCA notice that cites its EULA's ban on
reverse engineering, plus copyright and trade-secret law; the notice does not claim
circumvention ([NI-DMCA]). We did not use that repository. ConvertWithMoss, our main source,
remains public. Whether reading the owner's own libraries this way is acceptable is an
owner/legal question; this spec only records what public sources say.

**Evidence tags:**
- **[D]** documented by a public third-party source, not seen on files (source named);
- **[D?]** documented, but sources disagree or the source itself is unsure;
- **[N]** stated in NI's own public documentation (names only);
- **[I]** inferred (reasoning given);
- **[C]** confirmed on files: **nothing yet**;
- **[PH]** a Patternhaus decision.

**Cited sources:**
- [CWM-DOC] ConvertWithMoss format documentation, "Native Instruments Kontakt":
  https://github.com/git-moss/ConvertWithMoss/blob/main/documentation/README-FORMATS.md
- [CWM] ConvertWithMoss source, packages `format/ni/kontakt` (`Magic`, `KontaktDetector`,
  `kontakt1/Kontakt1Format`, `kontakt2/Kontakt2Header`, `kontakt2/Kontakt2Format`,
  `kontakt2/monolith/*`, `kontakt5/*`, `AbstractTagsAndAttributes`,
  `AbstractNKIMetadataFileHandler`, `kontakt2/K2Tag`), `format/ni/nicontainer`
  (`NIContainerItem`, `NIContainerDataChunk`, `NIContainerChunkType`, `chunkdata/*`),
  `file/ncw/NcwFile`, `file/StreamUtils`:
  https://github.com/git-moss/ConvertWithMoss/tree/main/src/main/java/de/mossgrabers/convertwithmoss
  (read for facts only; file names are given so a reviewer can check a fact).
- [NCW-RS] `ncw` crate documentation (MIT/Apache-2.0): https://docs.rs/crate/ncw/0.4.0 and
  its README https://docs.rs/crate/ncw/0.4.0/source/README.md
- [NI-MAN] Kontakt manual, "File formats":
  https://docs.native-instruments.com/online-guides/kontakt-manual/en/file-formats
- [VIC] VI-Control forum thread "Kontakt file extensions":
  https://vi-control.net/community/threads/kontakt-file-extensions.115663/ (read through a
  search summary; the page itself refused automated access).
- [NIFILE-FORK] README of a fork of the former ni-file project (container overview only):
  https://github.com/TriYop/ni-file (read through a search summary).
- [NI-DMCA] https://github.com/github/dmca/blob/master/2024/04/2024-04-04-native-instruments.md

Conventions: offsets in bytes; "u32 LE" etc. as in `flp.md`. Strings marked **UTF-16L** are
a u32 LE count of UTF-16 code units followed by that many UTF-16LE units, no terminator
[D, CWM StreamUtils]. Kontakt's internal byte order is little-endian unless stated; some
Kontakt 1–4 files are big-endian (PowerPC Macs) [D, CWM].

---

## 1. The file family [N, D]

| Ext | NI name [N] | What it holds [D unless marked] | M3 |
|---|---|---|---|
| `.nki` | Kontakt Instrument | one instrument (program); may be a **monolith** with samples inside [CWM] | **import** |
| `.nkm` | Kontakt Multi | up to 64 instruments plus MIDI/output setup [CWM-DOC] | **import** (each instrument separately) |
| `.nkb` | Kontakt Instrument Bank | a bank of instruments switched by program change [N name; patch type 2 in the K2 header, CWM] | detect, report "not supported yet" |
| `.nkp` | Kontakt Preset | a module preset (patch type 3) | refuse (not an instrument) |
| `.nkr` | Kontakt Resource File | library resources: scripts' data, IRs, graphics [VIC] | ignore |
| `.nkx` | Kontakt Monolith | a library's sample container [N name; VIC] | detect; read only if it is an unencrypted container of known layout (section 7) |
| `.nkc` | Kontakt Cache File | an index into an `.nkx`/`.nkr` monolith [VIC] | ignore (not needed) |
| `.ncw` | Kontakt Lossless Compression Audio File | one sample, lossless compressed [N name; D CWM, NCW-RS] | **decode** |
| `.nksn`, `.nkl` | Snapshot, Leap Kit [N] | | ignore |
| `.nicnt` | not in NI's list | library registration/metadata for Kontakt **Player** libraries [VIC] | licence signal (section 9.1) |

---

## 2. Identifying a file (dispatch on the first bytes) [D, CWM Magic/KontaktDetector]

Read the first 16 bytes. Check in this order:

| Test | Format | Section |
|---|---|---|
| bytes 12–15 = ASCII `hsin` | Kontakt 5–7 container (also 4.2.2+ variants; see 6) | 6 |
| bytes 0–15 = ASCII `/\ NI FC MTD  /\` (first four bytes `2F 5C 20 4E`) | Kontakt 5+ **monolith** file container | 7.2 |
| bytes 0–3 = `5E E5 6E B3` (instrument) or `5A E5 D6 A4` (multi) | Kontakt 1, little-endian | 3 |
| bytes 0–3 = `B3 6E E5 5E` (instrument) or `A4 D6 E5 5A` (multi) | Kontakt 1, big-endian | 3 |
| bytes 0–3 = `12 90 A8 7F` (instrument) or `01 EF 85 AB` (multi) | Kontakt 1.5 / 2–4.x, little-endian | 4 |
| bytes 0–3 = `7F A8 90 12` (instrument) or `AB 85 EF 01` (multi) | the same, big-endian | 4 |
| bytes 0–3 = `01 A8 9E D6` | NCW sample (the u32 LE value 0xD69EA801) | 8 |
| anything else | unknown: refuse with "not a Kontakt file we can read" | |

[CWM] writes the Kontakt 1/2 magics as 32-bit constants read in big-endian order from the
first four bytes; the byte sequences above are those constants in that order. **Kontakt
1.5** files carry the Kontakt 2 magic: if the u32 at offset 4 (in the file's byte order)
equals **0x24**, the file is laid out like Kontakt 1 (section 3), else like Kontakt 2
(section 4) [D, CWM KontaktDetector].

**Kontakt 8** files are not covered by any source we found [D: CWM lists 5–7]. Treat a
container whose versions exceed the known ranges (sections 6.4, 6.6) as "newer than
supported" and refuse with that message [PH].

---

## 3. Kontakt 1 and 1.5 (zlib-compressed XML) [D, CWM Kontakt1Format]

36-byte header, then a zlib stream:

| Off | Size | Field |
|---|---|---|
| 0 | 4 | magic (section 2) |
| 4 | u32 | offset of the zlib data = **0x24** |
| 8 | u16 | header version, 0x50 |
| 10 | u16 | unknown, 1 or 2 |
| 12 | 8 | zero |
| 20 | u32 | unknown, 1 |
| 24 | u32 | creation time, Unix seconds |
| 28 | u32 | total size of all used samples (raw PCM bytes) |
| 32 | u32 | zero |
| 36 | … | zlib stream (RFC 1950) holding the instrument **XML** |

Header fields use the file's byte order (section 2). The XML is described in section 5.
Kontakt 1 XML uses the `NiSS…` element family [D, CWM: a separate `NiSSTag` set] whose names
were not extracted for this edition; see Unknowns.

---

## 4. Kontakt 1.5 to 4.x with the "Kontakt 2" header [D, CWM Kontakt2Header/Kontakt2Format]

### 4.1 Header

After the 4-byte file magic, the header begins at **file offset 4** [I: CWM's 1.5 test reads
offset 4 for the zlib start, and its header reader begins with the compressed length].
Offsets below are file offsets under that assumption; fields use the file's byte order.

| Off | Size | Field |
|---|---|---|
| 4 | u32 | length of the compressed instrument data |
| 8 | u16 | header version: **0x100** = Kontakt 2 to 4.1 format, **0x110** = Kontakt 4.2 format (0x80 also exists, meaning unknown) |
| 10 | u32 | header magic: `0x722A013E` (Kontakt 2) or `0x1A6337EA` (Kontakt 4.2), each also seen byte-swapped in big-endian files |
| 14 | u16 | patch type: 0 NKM, 1 NKI, 2 NKB, 3 NKP, 4 NKG, 5 NKZ |
| 16 | u32 | application version: bytes = major, minor, sub-version, patch level (reversed in little-endian files); patch 0xFF = "see SVN field" |
| 20 | 4 | library id, ASCII: `Kon2`, `Kon3`, `Kon4`, `AkPi`, `ElPi` known |
| 24 | u32 | creation time, Unix seconds |
| 28 | u32 | unknown |
| 32 | u16 | number of zones |
| 34 | u16 | number of groups |
| 36 | u16 | number of instruments (modules in a multi) |
| 38 | u32 | total sample size (PCM bytes, no WAV headers) |
| 42 | u32 | **monolith flag: 1 = samples are inside this file** |
| 46 | u32 | minimum application version (same encoding as offset 16) |
| 50 | u32 | unknown |
| 54 | u32 | icon id |
| 58 | 8 | author, ISO-8859-1 |
| 66 | u8 ×3 | categories 1–3 (indices into fixed lists) |
| 69 | 86 | web site URL, ASCII; `(null)` = none |
| 155 | 3 | padding |
| 158 | u32 | flags (bit meanings unknown) |
| 162 | 16 (v0x110) or 4 (v0x100) | checksum: MD5 in 4.2, a 32-bit checksum before |
| 178 / 166 | u32 | SVN revision (patch level) |

The header counts (zones, groups) are a cheap cross-check for the import (M3 criterion 3
asks for "an independent reading of each file's metadata": these counts are one) [PH].

### 4.2 Kontakt 2 to 4.1 body (header version 0x100)

A **zlib** stream of the stated compressed length follows the header; it inflates to the
instrument **XML** (section 5) [D, CWM]. In a **monolith** (flag = 1), sample data follows
(section 7.1). An optional **soundinfo** block may follow the data: magic `0xB00EE1AE`,
version `0x000C0101`, a size, then UTF-8 XML with category, author and keyword metadata
[D, CWM Magic/Kontakt2Format].

### 4.3 Kontakt 4.2 body (header version 0x110)

After the header: `u32 CRC-32` of the compressed block, `u32 decompressed length`, 32 bytes
of padding, then the compressed block (length from offset 4), compressed with **FastLZ**
[D, CWM Kontakt2Format, Kontakt2Header]. It decompresses to **binary preset chunks**
(section 6.5), not XML. A CRC mismatch is logged by [CWM] but parsing continues;
**[PH]:** on a CRC mismatch or a FastLZ error, stop and refuse the file as "encrypted or
damaged" (section 9.4).

FastLZ is a public, MIT-licensed byte-oriented LZ77 format (https://github.com/ariya/FastLZ);
its level is in the top 3 bits of the first byte [I: FastLZ's own documentation]. T6 may use
a vetted crate or implement it from FastLZ's documentation.

---

## 5. The XML instrument (Kontakt 1, 1.5, 2–4.1) [D, CWM AbstractTagsAndAttributes, K2Tag, AbstractNKIMetadataFileHandler]

### 5.1 Structure

```
K2_Container
  Programs
    K2_Program  (name=…)
      Parameters  <V name="volume" value=…/> <V name="pan" …/> <V name="tune" …/> …
      Groups
        K2_Group (name=…, index=…)
          Parameters  <V …/>  (volume, pan, tune, releaseTrigger, keyTracking, reverse, voiceGroup, …)
          IntModulators  K2_IntMod … Envelope(type=ahd|ahdsr) <V attack|hold|decay|sustain|release|atkCurving …/> Targets/Target <V target=… intensity=…/>
          ExtModulators  K2_ExtMod … <V source=velocity|pitchBend|… intensity=…/> Targets/Target …
          GroupStart / StartCriteria … (round-robin "cycle…" modes, cycle class)
          Filter (Kontakt 1 only)
      Zones
        K2_Zone (groupIdx=…)
          Parameters <V rootKey lowKey highKey lowVelocity highVelocity fadeLowKey fadeHighKey fadeLowVelo fadeHighVelo sampleStart sampleEnd zoneVolume zonePan zoneTune …/>
          Sample (file / file_ex / file_ex2=…)
          Loops  Loop <V mode loopStart loopLength xfadeLength alternatingLoop loopTuning/>
K2_Bank (in multis: the bank holding several programs)
```

Values are `<V name="…" value="…"/>` elements under `Parameters` [D]. Exact nesting below
`K2_Group` and the full parameter list were not extracted; T6 should treat unknown `V`
names as data and report them [PH].

### 5.2 Zone values [D]

| Name | Meaning | Unit / conversion |
|---|---|---|
| `rootKey` | root key; negative → use `lowKey` | MIDI key |
| `lowKey`, `highKey` | key range | MIDI key |
| `lowVelocity`, `highVelocity` | velocity range | 1–127 |
| `fadeLowKey`, `fadeHighKey`, `fadeLowVelo`, `fadeHighVelo` | crossfade widths | semitones / velocity steps |
| `sampleStart`, `sampleEnd` | play range; used only when end > start | frames |
| `zoneVolume` | linear gain (gain dB = 20·log10(zone × group × program)) | linear |
| `zonePan` | pan, normalised −1…1 (sum of zone, group, program, clamped) | |
| `zoneTune` | tuning; [CWM] combines zone, group and program tune into −1…1 meaning ±100 semitones for this family | **[D?]** the exact scale per version is not clear |
| `groupIdx` (attribute) | the group this zone belongs to: equals a group's `index` | |
| `reverse` | reverse playback | `yes` = true |

**Sample reference** [D]: the zone's `Sample` element has a file attribute (`file`,
`file_ex`, or `file_ex2`, the newest). Values without a leading marker are plain relative
paths with `\` separators. Values with a leading marker are an encoded path made of
segments: volume/disk, parent folder, folder (with a 3-digit length prefix), file name
(with a fixed-length prefix), and `%XXXX` escapes for non-ASCII characters [D, CWM]. The
exact marker characters were not extracted; this must be confirmed on files (section 11).
Library references (NKS libraries) are not supported by [CWM] and refused [PH: report
"library-relative path"].

### 5.3 Loops [D]

`Loops/Loop` with values: `mode` = `until_end`, `until_release`, or `oneshot`;
`loopStart` (frames); `loopLength` (frames; loop end inclusive = start + length − 1);
`xfadeLength` (frames); `alternatingLoop` (`yes` = ping-pong; only for the two loop modes);
`loopTuning` (a ratio: semitones = 12·log2(value)). A `oneshot` loop marks the zone
one-shot and adds no loop. A zone may have several loops [D].

### 5.4 Group values [D]

`volume` (linear), `pan`, `tune`, `keyTracking`, `reverse`, `releaseTrigger` (`yes` = the
group plays on note-off), `voiceGroup` (exclusive group; −1 = none), and start criteria:
modes whose name starts with `cycle` make the group a round-robin member, with its
position from the cycle class [D, CWM K2MetadataFileHandler].

### 5.5 Program values and modulators [D]

Program `volume`, `pan`, `tune` fold into every zone when the file is a single instrument.
Envelope element `Envelope type=ahd|ahdsr` with `attack`, `hold`, `decay`, `release` in
**milliseconds** and `sustain` as a level; `atkCurving` = attack curve; `noteOffLessMode`
= ignore note-off. Targets: `volume`, `pitch`, `filterCutoff`; intensity numeric. External
modulator with source `velocity` → velocity-to-volume depth; source `pitchBend` →
intensity × 1200 cents (clamped ±9600) as bend range [D, CWM]. Kontakt 1 groups carry a
`Filter` element (type `lp|hp|bp` + pole count; cutoff normalised 0–1 over ~21.8 kHz;
resonance) [D].

---

## 6. Kontakt 5–7: the NI container (`hsin`) [D, CWM nicontainer; NIFILE-FORK for the overview]

### 6.1 Item

A container **item** is the whole file at top level; items nest [D].

| Order | Size | Field |
|---|---|---|
| 1 | u64 LE | item length **including** these 8 bytes |
| 2 | u32 | header version = 1 |
| 3 | 4 | ASCII `hsin` |
| 4 | u32 | unknown, always 1 |
| 5 | u32 | unknown, always 0 |
| 6 | 16 | item UUID |
| 7 | … | the item's **data-chunk chain** (6.2) |
| 8 | u32 | item version |
| 9 | u32 | number of child items |
| 10 | … | each child: a child wrapper holding a nested item [D: CWM `NIContainerChildItem`; wrapper fields not extracted — see Unknowns] |

So `hsin` sits at file offset 12 [D, CWM KontaktDetector].

### 6.2 Data chunk chain

Each data chunk [D]:

| Order | Size | Field |
|---|---|---|
| 1 | u64 LE | chunk length including these 8 bytes |
| 2 | 4 | ASCII domain id [D, CWM]; NIFILE-FORK calls data segments `dsin` and says tags are spelt backwards and that product-specific tags exist (e.g. for Kontakt 4) [D?]. Do not rely on a fixed value; read and report it |
| 3 | u32 | chunk type id (6.3) |
| 4 | u32 | chunk version = 1 |
| 5 | … | **the next data chunk**, a complete chunk of this same form, unless this chunk's type is 1 (terminator) |
| 6 | … | this chunk's payload: the rest of its length |

The chain therefore nests each chunk inside the previous one; the terminator (type 1)
ends it.

### 6.3 Chunk types [D, CWM NIContainerChunkType]

| Id | Name | Use for import |
|---|---|---|
| 1 | Terminator | end of chain |
| 3 | BNI Sound Preset | (Kontakt 4 domain only) |
| 4 | BNI Sound Header | (Kontakt 4 domain only) |
| 100 | Bank | |
| 101 | Authoring Application | **must name Kontakt**, else refuse "made by <app>" |
| 102 | Bank Container | |
| 103 | Preset Container | |
| 104 | Binary Chunk Item | |
| 106 | **Authorization** | licence check (9.2) |
| 108 | Soundinfo Item | metadata (author, category, keywords) |
| 109 | **Preset Chunk Item** | the instrument data (6.4) |
| 110 | External File Reference | |
| 111 | Resources | |
| 112 | Audio Sample Item | |
| 113 | Internal Resource Reference Item | |
| 114 | Picture Item | |
| 115 | **Subtree Item** | a nested container, possibly compressed (9.3) |
| 116 | **Encryption Item** | encryption metadata (9.3) |
| 117 | App Specific | |
| 118 | Container Root | |
| 120 | Automation Parameters | |
| 121 | Controller Assignments | |
| 122 | Module | |
| 123 | Module Bank | |

Unknown ids: [CWM] treats them as errors. **[PH]:** skip by length and report.

Each chunk payload begins with a version field [D: CWM `readVersion`; width assumed u32 [I]].

### 6.4 Preset Chunk Item payload (type 109) [D, CWM PresetChunkData]

| Field | Size |
|---|---|
| version | u32 [I] |
| dictionary type (or an authorisation checksum; unclear) | u32 |
| number of items, must be 1 | u32 |
| size of the preset data | u32 |
| unknown | u32 |
| **preset data** | *size* bytes |
| padding | u32 |
| magic `0x8565620D` (present "even for encrypted files", so it is not a protection signal) | u32 |

The preset data is a sequence of **preset chunks** (6.5), stored without further
compression in the cases [CWM] handles.

### 6.5 Preset chunks (Kontakt 4.2 and 5–7) [D, CWM KontaktPresetChunk/KontaktPresetChunkID]

Every preset chunk: `u16 id`, `u32 size`, then `size` bytes of body (LE).

Body by id:
- **Structured** (BANK, PROGRAM_CONTAINER, PROGRAM, PAR_SCRIPT, PAR_FX_SEND_LEVELS,
  VOICE_GROUPS, PARAMETER_ARRAY_8, INSERT_BUS, SAVE_SETTINGS, QUICK_BROWSE_DATA): `u8 flag`;
  if 0, the rest is raw data; else `u16 version`, `u32 private size` + private bytes,
  `u32 public size` + public bytes, `u32 children size` + children (a sequence of preset
  chunks).
- **Lists** (GROUP_LIST, ZONE_LIST): `u32 count`, then `count` entries, each a structured
  body. In a ZONE_LIST each entry is preceded by a `u32` reference value used as that
  zone's id; group-list entries are groups.
- **Fixed parameter arrays** (PARAMETER_ARRAY_16, PARAMETER_ARRAY_32): `u8 0`, `u16
  version`, then for each of the 16 or 32 slots a `u8 present` flag, followed (if set) by a
  child chunk (`u16 id`, `u32 size`, data).
- **Raw** (SLOT_LIST, PAR_FX, FILENAME_LIST, FILENAME_LIST_EX, unknown ids): the body is
  data.

Chunk ids:

| Id | Name | Id | Name |
|---|---|---|---|
| 0x00 | PAR_MOD_BASE | 0x37 | SLOT_LIST |
| 0x03 | BANK | 0x38 | STAR_CRIT_LIST |
| 0x04 | GROUP | 0x39 | LOOP_ARRAY |
| 0x06 | PAR_SCRIPT | 0x3A | PARAMETER_ARRAY_8 |
| 0x0C | PAR_EXTERNAL_MOD | 0x3B | PARAMETER_ARRAY_16 |
| 0x0D | PAR_INTERNAL_MOD | 0x3C | PARAMETER_ARRAY_32 |
| 0x17 | PAR_FX_SEND_LEVELS | 0x3D | FILENAME_LIST |
| 0x25 | PAR_FX | 0x3E | OUTPUT_CONFIGURATION |
| 0x28 | PROGRAM | 0x45 | INSERT_BUS |
| 0x29 | PROGRAM_CONTAINER | 0x47 | SAVE_SETTINGS |
| 0x2C | ZONE | 0x48 | MULTI_CONFIGURATION |
| 0x32 | VOICE_GROUPS | 0x4A | PAR_GROUP_DYNAMICS |
| 0x33 | GROUP_LIST | 0x4B | FILENAME_LIST_EX |
| 0x34 | ZONE_LIST | 0x4E | QUICK_BROWSE_DATA |
| 0x35 | PRIVATE_RAW_OBJECT | | |
| 0x36 | PROGRAM_LIST | | |

Tree: PROGRAM chunks may appear at any depth; a top-level BANK marks a multi, with
SLOT_LIST (its programs) and MULTI_CONFIGURATION children; FILENAME_LIST or
FILENAME_LIST_EX at top level lists the sample files [D, CWM KontaktPresetAccessor].

`PAR_SCRIPT` holds the instrument's KSP scripts: keep as data for M3b (M3.md decision 1);
report "scripts not run yet" [PH].

### 6.6 Program (PROGRAM public data) [D, CWM Program]

In order: name (UTF-16L); total sample size (f64); MIDI transpose (i8); volume, pan, tune
(3 × f32 LE); clip limits (u8 ×4: velocity low, velocity high, key low, key high); default
keyswitch (u16); preload size, library id, fingerprint, loading flags (4 × u32); group-solo
flag (u8); icon (u32); credits, author, URL (3 × UTF-16L; `(null)` = none); three category
indices (3 × u16); then trailing bytes kept as data (8 bytes from 5.3, 20 from 5.4, 24 in 7).

Program chunk versions [D]: 0xA5 (Kontakt 5.3.0), 0xA8 (5.4.3–5.5.2), 0xAB (5.6.8–5.8.1),
0xAE (6.5.2–6.8.0), 0xAF (7.1.3–7.5.1), 0xB1 (7.6.0–7.6.1).

Children: GROUP_LIST (groups), ZONE_LIST (zones); VOICE_GROUPS, PARAMETER_ARRAY_8,
PAR_SCRIPT, PAR_MOD_BASE, INSERT_BUS, QUICK_BROWSE_DATA are not needed for zones.

### 6.7 Group (GROUP public data) [D, CWM Group]

In order: name (UTF-16L); volume, pan, tune (3 × f32, same scales as zones, 6.8); key
tracking (u8 bool); reverse (u8); release trigger (u8); release trigger monophonic (u8);
release-trigger counter (i32); MIDI channel (i16, −1 = any); voice group index (i32,
−1 = none); FX amplitude split point (i32); muted (u8); soloed (u8); interpolation quality
(i32). Group chunk versions above 0x9C are unknown [D].

Group children: PARAMETER_ARRAY_16 holds the **internal modulators** (envelopes, LFOs),
PARAMETER_ARRAY_32 the **external modulators** (velocity, CC, pitch bend …) [D]. Their
inner layout was not extracted (M3 scope puts envelopes and filters after zones, loops, NCW
and monoliths: DESIGN §8). Round-robin start criteria live in STAR_CRIT_LIST [I from the
name]; not extracted.

### 6.8 Zone (ZONE public data) [D, CWM Zone]

Read in order (LE):

| # | Type | Field | Notes |
|---|---|---|---|
| 1 | u32 | sample start, frames | |
| 2 | u32 | sample end, stored as an offset **back from the last frame** (end = frames − value) | [D] |
| 3 | u32 | start-modulation range | ignore |
| 4 | u16 | velocity low (1–127) | |
| 5 | u16 | velocity high | |
| 6 | u16 | key low (0–127) | |
| 7 | u16 | key high | |
| 8 | u16 | velocity fade low (velocity steps) | |
| 9 | u16 | velocity fade high | |
| 10 | u16 | key fade low (semitones) | |
| 11 | u16 | key fade high | |
| 12 | u16 | root key | |
| 13 | f32 | volume, linear (1.0 = 0 dB; 0.015625 = −36 dB; 64 = +36 dB) | |
| 14 | f32 | pan −1…1 | |
| 15 | f32 | tune as a **ratio** 2^(semitones/12), range 0.125–8 (±36 semitones) | |
| 16 | 2 × u8 | *only in zone version 0x9A+*: unknown | |
| 17 | u32 | *only in 0x9A+*: unknown (0xFFFFFFFF) | |
| 18 | u32 | **file-name index** into the file list (6.9); −1 = none | |
| 19 | u32 | bytes per sample: 1, 2, 3, 4 | |
| 20 | u32 | sample rate, Hz | |
| 21 | u8 | channels | |
| 22 | u32 | frames | |
| 23 | u32 | unknown (44 seen) | |
| 24 | u32 | *only in versions ≤ 0x93*: unknown | |
| 25 | u32 | recording root note; 0 = ignore | |
| 26 | f32 | tuning, always 1.0 seen | |
| 27 | u8 | unknown | |
| 28 | u32 | sample size = frames × channels × bytes per sample | |

Zone versions [D]: **0x93** (Kontakt 4.2.x: has field 24, lacks 16–17); **0x98**
(5.3.0–5.6.8) and **0x99** (5.8.1): neither; **0x9A** (6.5.2–6.8.0): adds 16–17. Above
0x9C: unknown → refuse "newer than supported" [PH].

The **group** a zone belongs to is not a field of the zone data [D: CWM passes it in from
outside, and its commented-out mapping points to a group index stored with the zone]
**[D?]**. Candidate: the u32 reference that precedes each ZONE_LIST entry (6.5). This is
the single most important fact to confirm on files (section 11).

**Loops** are in the zone's LOOP_ARRAY child [D, CWM Program, ZoneLoop]: `u16` bitmask
(bit *i* = loop *i* present, up to 8), then for each present loop `u16` marker **0x60**
followed by:

| Type | Field |
|---|---|
| i32 | mode: 1 until end, 2 until release, 3 one-shot (0 or other = off [I]) |
| u32 | loop start, frames |
| u32 | loop length, frames |
| u32 | loop count (0 = infinite [I]) |
| u8 | alternating (ping-pong) if > 0 |
| f32 | loop tuning ratio (semitones = 12·log2) |
| u32 | crossfade length, frames |
| u8 | padding, after every loop except the last |

### 6.9 File list (FILENAME_LIST 0x3D, FILENAME_LIST_EX 0x4B) [D, CWM FileList]

FILENAME_LIST_EX starts with `u16 version` (2 or 3 accepted).
- **Version 2 and plain FILENAME_LIST:** a "special files" list, then the **sample files**
  list, then per sample file a timestamp (u32 Unix s) + 4 zero bytes; EX only: per sample
  file a u32 (unknown) and then an "other files" list; plain only: a final u32.
- **Version 3:** one list used for samples: `i32 count`, 8 zero bytes, then `count − 1`
  paths, each followed (if data remains) by 4 zero bytes, a timestamp and 20 zero bytes;
  24 zero bytes end it.
- **List (v2):** `i32 count`, then `count` paths.
- **Path:** `u32 segment count`, then segments, each `u8 type` + payload:

| Type | Meaning | Payload | Path text |
|---|---|---|---|
| 0 | drive letter | 2 ASCII bytes | `X:/`, or `/` if empty |
| 1 | drive letter | UTF-16L | same |
| 2 | folder | UTF-16L | name + `/` |
| 3 | parent folder | none | `../` |
| 4 | file name | UTF-16L | name |
| 6 | top-level marker | none | nothing |
| 8 | `.nkx` file name | UTF-16L | name |
| 9 | `.nkm` file name | UTF-16L | name |

Concatenate segments in order. A path that contains an `.nkx` segment refers to a sample
**inside a library monolith** (section 7.3) [I].

Zone field 18 indexes the sample-files list (0-based) [D].

---

## 7. Monoliths

### 7.1 Kontakt 2–4 monolith (`.nki` with monolith flag 1) [D, CWM Kontakt2Monolith]

After the header, the file is a series of blocks, each starting with a u32 magic in the file's
byte order:

| Magic | Block | Layout |
|---|---|---|
| 0x5E70AC54 | resource **directory** (NKR header) | entries (below) |
| 0x2AE905FA | wallpaper | 14-byte preamble, u64 length, image (skip) |
| 0x4916E63C | the instrument (NKI) | the Kontakt 2 header + zlib XML; the end of the sample area |
| 0x16CCF80A | sample | u16 version, u32 length, sample data (WAV or NCW bytes) |
| 0x0040179F | "raw" sample | no length: data runs to the next sample magic followed by version 0x110 |

Directory entry: `u16 length` (whole entry, incl. 8 header bytes), `u32 pointer` (absolute
file offset [I]), `i16 reference type` (0–4; named types include SAMPLE and NKI), then
`length − 8` bytes of UTF-16 name [D]. An entry typed NKI whose name ends in `.wav`, `.ncw`
or `.aif` is really a sample (a known third-party writer bug) [D].

Samples are matched to directory SAMPLE entries **by order**, not by a stored reference
[D, CWM] **[D?]** (the pointer may be the real link; confirm on files). A sample's name is
its entry name; names ending `.ncw` are NCW, others WAV. Zones find their sample by the
file name in their path (5.2) [D].

### 7.2 Kontakt 5+ monolith file container [D, CWM Kontakt5MonolithFormat]

| Part | Layout |
|---|---|
| header | 16 ASCII bytes `/\ NI FC MTD  /\`; 248 bytes padding; 8 bytes of 0xF0; u64 file count; u64 total size |
| table of contents | 16 ASCII bytes `/\ NI FC TOC  /\`; 600 unknown bytes; per file a **640-byte entry**: u64 index (1-based), 16 unknown bytes, **600-byte UTF-16LE file name** (trim), u64 unknown, **u64 end offset** of the file's data; then 8 bytes of 0xF1, 16 unknown bytes, the TOC string again, 592 padding bytes |
| data | files back to back, in index order; file *i* runs from the previous end offset (0 for the first) to its end offset, relative to the start of the data area |

The instrument is the first entry whose name ends `.nki` (or `.nkm` for a multi); it is a
Kontakt 5+ container (section 6). Entries ending `.wav` or `.ncw` are samples; zones find
them by file name [D].

**Streaming [PH]:** never read the whole monolith into memory (CWM does; libraries can be
many GB). Index the TOC, then stream each sample's byte range in place through the M2
streamer; NCW samples need a seekable block decoder (section 8 makes blocks independently
decodable).

### 7.3 Library monoliths (`.nkx`) and caches (`.nkc`)

[N] names `.nkx` "Kontakt Monolith"; [VIC] says `.nkc` stores pointers to files inside an
`.nkx` or `.nkr`. No source we found documents the `.nkx` layout [D: none]. **[PH]:** if an
`.nkx` begins with the `/\ NI FC MTD  /\` header, read its TOC as in 7.2; otherwise refuse
"library monolith format not supported". Instruments of libraries that ship `.nkx`
containers are typically Player libraries (9.1), which are refused before this point.

---

## 8. NCW lossless sample codec [D, CWM NcwFile; NCW-RS agrees on the scheme]

### 8.1 File header (120 bytes, LE)

| Off | Size | Field |
|---|---|---|
| 0 | u32 | magic 0xD69EA801 (bytes `01 A8 9E D6`) |
| 4 | u32 | version: 0x130 (Kontakt 4) or 0x131 (Kontakt 5) |
| 8 | u16 | channels |
| 10 | u16 | bits per sample: 16, 24 or 32 |
| 12 | u32 | sample rate, Hz |
| 16 | u32 | frames (samples per channel) |
| 20 | u32 | offset of the block-offset table |
| 24 | u32 | offset of the block data |
| 28 | u32 | total size of the blocks (informational) |
| 32 | 88 | reserved |

### 8.2 Block-offset table

At the table offset: `(data offset − table offset) / 4` u32 entries, each the offset of a
block relative to the data offset. The first entry is 0; consecutive differences give block
sizes; the last entry marks the end [D, CWM]. **Each block holds 512 frames** per channel
[D ×2: CWM, NCW-RS 0.4.0; earlier NCW-RS versions assumed 64, now corrected]. The final
block is truncated to the frame count.

### 8.3 Block

A block is one sub-block per channel, in channel order. Each sub-block: 16-byte header, then
data:

| Off | Size | Field |
|---|---|---|
| 0 | u32 | block magic: CWM gives the value 0x3E9A0C16, NCW-RS gives 0x160C9A3E **[D?]** (the same four bytes read in opposite orders; confirm the byte sequence on a file) |
| 4 | i32 | base value: the first sample |
| 8 | i16 | **bits**: sign selects the mode, magnitude the width |
| 10 | u16 | flags: bit 0 = **mid/side** stereo, bit 1 = **IEEE float**; 0–3 valid |
| 12 | 4 | padding |
| 16 | 512 × \|bits\| / 8 bytes (or 512 × file bit depth / 8 when bits = 0) | samples |

Decoding one channel of a block [D ×2]:
- **bits > 0: delta mode.** 512 signed values of `bits` width, packed LSB-first,
  sign-extended. Sample 0 = base value; sample *i* = sample *i−1* + delta *i−1* (the last
  delta is unused). Accumulate with wrapping 32-bit arithmetic [D, NCW-RS].
- **bits < 0: truncated PCM.** 512 signed samples of `|bits|` width, packed LSB-first,
  sign-extended (use 64-bit intermediates for 32-bit) [D, CWM].
- **bits = 0: raw PCM** at the file's bit depth, little-endian [D ×2].
- **Mid/side** (flag bit 0, stereo only): channel 0 = mid, channel 1 = side; left = mid +
  side, right = mid − side [D, CWM]. Other channel counts with this flag: invalid.
- **Float** (flag bit 1): the decoded 32-bit integers are IEEE-754 single bit patterns;
  mid/side then works on the float values [D, CWM].

Output: interleaved frames at the file's bit depth (or f32). NCW has no encryption field
[D: none of the sources mention one]; a wrong magic means "not NCW".

---

## 9. Encrypted and licence-locked content: detection only

The importer checks these **in order** and refuses at the first match, with a plain
message. It never inspects, logs or retains the protected bytes beyond what the check reads.

### 9.1 Library level [PH, from D VIC]
- A library folder (the instrument's folder or any ancestor up to the library root, i.e. the
  folder holding `Instruments/` or `Samples/`) that contains a **`.nicnt`** file is a
  Kontakt **Player** library (registered through Native Access) [D, VIC]. **[PH]:** refuse
  every instrument in it: "This is a Kontakt Player library (licence-locked). Patternhaus
  does not open these." *Owner sign-off item:* this refuses some libraries whose files
  might be readable; that is deliberate (DESIGN §8 boundary covers licence-locked as well as
  encrypted).

### 9.2 Kontakt 5–7 container (section 6)
- An **Authorization** chunk (type 106) whose payload has `pidContent == 1` and a non-empty
  list of product ids (serial-number PIDs) ⇒ **licence-locked** [D, CWM
  AuthorizationChunkData/Kontakt5Format: CWM logs that the container includes an encrypted
  sub-tree and produces nothing]. The payload layout needed for this check: version (u32
  [I]), `u32 pidContent`; if 1: `u32 number of licence blocks` (= 1), `u32 number of PIDs`,
  then that many UTF-16L strings. Stop reading there. `pidContent == 2` carries no PIDs.
- An **Encryption Item** chunk (type 116) anywhere ⇒ **encrypted** [I: its name and
  presence; CWM keeps it opaque].
- A **Subtree Item** chunk (type 115) whose payload says "compressed" (`u8 > 0` after the
  version) and whose FastLZ data does not decompress cleanly ⇒ **encrypted** [D, CWM
  SubTreeItemChunkData infers encryption from this failure].
- An **Authoring Application** chunk that does not name Kontakt ⇒ refuse as "not a Kontakt
  instrument" (not a protection signal).

### 9.3 Kontakt 4.2 (section 4.3)
- CRC-32 mismatch or FastLZ failure on the instrument block ⇒ refuse "encrypted or damaged"
  [PH]. (No source documents a protection flag in this header; the flags field's bits are
  unknown [D].)

### 9.4 Kontakt 1.5–4.1 (section 4.2) and 1 (section 3)
- zlib failure, or inflated data that is not well-formed XML starting with `<` ⇒ refuse
  "encrypted or damaged" [PH].

### 9.5 Everything
- Any parse that would require reading past an Authorization/Encryption signal is not
  attempted. The report counts such files as "refused: encrypted/licensed" (M3
  criterion 3 report wording) [PH].

---

## 10. Version summary

| Kontakt version | Container | Instrument data | Samples | Monolith |
|---|---|---|---|---|
| 1 | K1 header (36 B), magic `5E E5 6E B3` / `5A E5 D6 A4`, LE or BE | zlib XML (NiSS family) | external WAV/AIFF | — |
| 1.5 | K2 magic, offset 4 = 0x24 → K1 layout | zlib XML | external | — |
| 2, 3, 4.0–4.1 | K2 header v0x100, header magic 0x722A013E, LE or BE | zlib XML (`K2_*`), `file_ex2` paths | external WAV/AIFF/NCW (NCW from Kontakt 4 [D: version 0x130]) | flag in header; NKR directory + sample blocks (7.1) |
| 4.2.x | K2 header v0x110, header magic 0x1A6337EA, MD5 | CRC-32 + FastLZ → preset chunks; zone v0x93 | external, NCW | as 7.1 [I] |
| 5.x | `hsin` container | preset chunks; program 0xA5–0xAB; zone 0x98/0x99 | NCW v0x131 | `/\ NI FC MTD  /\` (7.2) |
| 6.x | `hsin` | program 0xAE; zone 0x9A | NCW | 7.2 |
| 7.x | `hsin` | program 0xAF, 0xB1; zone 0x9A (later unknown) | NCW | 7.2 |
| 8.x | unknown | unknown | unknown | unknown |

---

## 11. Unknowns, and facts that need real files

Everything in this document is **[D]** or weaker. Before T6 relies on it, the owner's
unencrypted libraries (read-only, in place, nothing committed) should confirm, in priority
order:

1. **Zone → group link** in Kontakt 5+ (6.8): the ZONE_LIST reference value or another field.
   Check: zones' key ranges grouped by candidate value match the groups Kontakt shows.
2. Byte sequence of the NCW **block magic** (8.3) and the u64 length semantics (6.1, 6.2).
3. **Data chunk payload version width** (assumed u32) and the child-item wrapper (6.1).
4. **K2 header base offset** (4.1: assumed file offset 4) and the 0x80 header version.
5. **`file_ex2` path markers** (5.2).
6. Zone **tune** scale in the XML family (5.2) and sample-end encoding in 5+ (6.8 field 2).
7. Loop count and mode-0 meaning (6.8).
8. K2-4 monolith: sample-to-entry pairing by order vs by pointer (7.1).
9. Kontakt 1 XML (`NiSS`) element names (3).
10. STAR_CRIT_LIST (round robin, keyswitch start criteria) and the internal/external
    modulator layouts in PARAMETER_ARRAY_16/32 (6.7), for the later envelope/filter phase.
11. Kontakt 7.6+ and 8 versions (10).
12. Whether `.nkx` files of non-Player libraries use the FC MTD container (7.3).

**How to collect evidence without leaking content:** a read-only probe (to be written in
T6 or T8 next to `tools/flp-probe`) that prints, per file: detected family, header fields
(counts, versions, monolith flag), chunk ids and sizes, and **aggregate** statistics only,
like `flp-probe stats`. It must stop at any section 9 signal and never print preset bytes,
names of protected items, or sample data. Its output goes to `local/reports/`, never into
the repo; only library names and pass/fail go into `docs/parity/kontakt-corpus.md`.

---

## 12. Fixture plan (generated, never copied)

No NI file may enter the repo. All fixtures are written by test code from this spec
(`build::KontaktBuilder` in T6, like `FlpBuilder`). They test **agreement with this
document**; real compatibility is M3 criterion 3 (owner corpus).

| Id | Content | Exercises |
|---|---|---|
| F-NKI-01 detect | 16-byte prefixes of every family, both byte orders, NCW, FC MTD, garbage | dispatch (section 2) |
| F-NKI-02 k1 | K1 header + zlib XML: 1 group, 3 zones, loops | section 3, 5 |
| F-NKI-03 k15 | K2 magic with 0x24 at offset 4 | 1.5 rule |
| F-NKI-04 k2 | K2 header v0x100 + zlib XML: 2 groups (`groupIdx`), velocity layers, crossfades, release-trigger group, `cycle` round robin, plain and encoded `file_ex2` paths | sections 4.2, 5 |
| F-NKI-05 k2-be | the same, big-endian | byte order |
| F-NKI-06 k2-mono | K2 monolith: directory, two sample blocks (one WAV, one NCW), NKI block | 7.1 |
| F-NKI-07 k42 | K2 header v0x110, CRC-32 + FastLZ preset chunks with zone v0x93 | 4.3, 6.5, 6.8 |
| F-NKI-08 k5 | `hsin` container, authoring app = Kontakt, preset chunk item, program 0xA5, zones 0x98 with loops, FILENAME_LIST_EX v2 | section 6 |
| F-NKI-09 k6/k7 | the same with program 0xAE / 0xB1 and zone 0x9A; FILENAME_LIST_EX v3 | version branches |
| F-NKI-10 k5-mono | FC MTD container holding the F-NKI-08 instrument and two samples | 7.2, streaming by byte range |
| F-NKI-11 nkm | a K5 multi with BANK, SLOT_LIST of two programs | multis |
| F-NKI-12 refuse | synthetic containers with (a) an Authorization chunk, pidContent 1 and one PID string "TEST"; (b) an Encryption Item chunk with zero-filled payload; (c) a compressed Subtree Item with random bytes; (d) a K4.2 file with a wrong CRC; (e) a folder with an empty `x.nicnt` | detection (section 9) — synthetic markers only, **no real protected data** |
| F-NCW-01 ncw | NCW written by test code from soundgen tones: mono/stereo, 16/24/32-bit, each bits mode (+, −, 0), mid/side, float flag, partial last block | section 8; bit-exact against the source PCM |
| F-NKI-13 malformed | truncated chunks, lengths past end, unknown chunk ids, zone version 0x9D, counts that disagree | robustness (fuzz seeds) |
