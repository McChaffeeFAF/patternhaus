# FLP corpus report (aggregate)

**What this is.** Aggregate statistics over the owner's local FL Studio projects, gathered
for `flp.md` (M2/T4) with `tools/flp-probe stats` and throwaway scripts. It contains numbers
only: no file, song, sample, user or plugin names, and nothing from inside any file. The
files themselves stay on the owner's machine and are never committed.

Corpus: the two default FL projects folders (Documents and OneDrive Documents), 255 `.flp`
files, analysed 2026-10-11.

## Files

| | Count |
|---|---|
| Files | 255 |
| Parsed to the exact end of the data chunk | 255 |
| Trailing bytes after the data chunk | 0 in every file |
| Header body length 6 | 255 |
| Header format 0 (project) | 255 |
| PPQ 96 | 255 |
| Header channel count = number of channel events | 255 |
| Total events | 899 637 |

## FL versions

| Version (event 199) | Files |
|---|---|
| 21.2.2 (build 3914) | 251 |
| 20.7.2 (build 1852) | 3 |
| 12.0.1 | 1 |

## Event-id coverage

106 distinct event ids occur. By size class: byte (0–63) 26 ids, word (64–127) 19, dword
(128–191) 23, variable (192–255) 38.

- In every file: 0, 9, 10, 11, 17, 18, 20, 21, 22, 23, 28, 29, 31, 64, 67, 69, 70, 71, 74, 75,
  76, 80, 83, 85, 86, 89, 97, 128, 131, 132, 133, 138, 139, 143, 144, 145, 146, 147, 154, 155,
  156, 194, 195, 199, 200, 201, 202, 203, 206, 207, 209, 212, 215, 216, 218, 219, 221, 225, 226,
  228, 229, 231, 233, 235, 236, 237, 238.
- In all FL 20/21 files: 32, 35–39, 98, 99, 100, 159, 241 (and the FL 21-only ids below in all
  FL 21 files).
- FL 21 only: 40, 41, 42, 43, 44, 47, 165, 166, 167, 242, 243.
- Rare: 65/224 (44 files), 95 (212), 149 (216), 204 (216), 196 (251 files), 213 (253), 150/157/158/164/193
  (39), 234/227 (28 files, 32 events), 223 (2), 239 (1).
- Never seen although documented: 2, 3, 72, 73 (old channel volume/pan), 66, 93 (old tempo),
  94 (layer children), 192 (old channel name), 197, 198.

Text: every text-bearing variable event is UTF-16LE with a two-byte terminator, except 199
(ASCII) in all versions.

## Features

| Feature | Occurrence |
|---|---|
| Channels per file | 1–36, median 4; 1420 in total |
| Channel types | audio clip 686, generator plugin 425 (421 VST host, 4 native), sampler 277, automation clip 32, layer 0 |
| Channels with a sample path | 922 (275 with a placeholder prefix, 647 absolute) |
| Files with patterns | 44 (up to 39 patterns per file) |
| Notes | 38 989 in 335 note events |
| Note fields at default everywhere | fine pitch, pan, mod X, mod Y, group, flags |
| Playlist clips | 4 187: 2 499 pattern clips and 1 688 channel clips (audio/automation); 3 664 in 60-byte FL 21 records, 523 in 32-byte FL 12/20 records |
| Arrangements | exactly one per FL 20/21 file |
| Named playlist tracks | 1 |
| Mixer insert records | 127 per FL 20/21 file, 105 in the FL 12 file |
| Named inserts | 1312 in 216 files |
| Coloured inserts | 1311 |
| Effect plugins in slots | 3456 in 251 files (0–57 per file, median 15) |
| VST host instances (channels + slots) | 901: VST3 432, VST2 340, no plugin data 129 |
| Native plugin instances | 2 980 (channels and slots) |
| Files with automation clips | 28 (32 clips, 2–28 points each); targets seen: mixer insert volume (24), the 0x4000xxxx space (8) |
| Tempo | 76–151 BPM, 12 distinct values; 3 files with a fractional tempo |
| Time signature | 4/4 in every file |

## Fixed-size record checks

| Event | Record | Checked |
|---|---|---|
| 224 notes | 24 bytes | 335 events, all multiples; channel field matched a channel id in 38 989 of 38 989 notes |
| 225 mixer params | 12 bytes | 255 events, 1 138 230 records |
| 233 playlist | 60 bytes (FL 21), 32 (FL 12/20) | 255 events, 4 187 records; every channel clip's index matched a channel id |
| 234 automation | 21 + 24·n + 112 | 32 events |
| 213 VST wrapper | id/u64-length records | 901 of 901 parse to the end |
