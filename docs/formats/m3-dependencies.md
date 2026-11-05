# M3 dependencies (Haus Sampler)

Status: **M3/T0 check, 2026-11-05.** What the M3 tracks need from crates.io, with licences
checked against `deny.toml`. The rule from the brief: prefer small parsers of our own, and add
**no** heavy dependency in T0. T0 added no crate to `Cargo.lock`; it only made `ph-formats`
use `toml`, which the lock file already held (through `ph-ui`).

Licence policy (`deny.toml`): MIT, Apache-2.0, BSD, ISC, Zlib, 0BSD, CC0, BSL and Unicode
licences are allowed everywhere; MPL-2.0 only for the named, unmodified `symphonia` crates;
GPL/LGPL/AGPL denied.

## Needed, already in the tree

| need | crate | version in `Cargo.lock` | licence | `deny.toml` | used by | notes |
|---|---|---|---|---|---|---|
| `instrument.toml` read/write | `toml` (+ `toml_parser`, `toml_writer`, `toml_datetime`, `serde_spanned`, `winnow`) | 1.1.8 | MIT OR Apache-2.0 (winnow: MIT) | allowed | `ph-formats` (M3/T0), `ph-ui` | Now a direct dependency of `ph-formats`. Pure Rust. |
| SF3 sample data (Ogg Vorbis) | `symphonia-format-ogg`, `symphonia-codec-vorbis` | 0.5.5 | MPL-2.0 | named exceptions (unmodified) | `ph-formats::decode` | Already decodes `.ogg` files; T4 feeds it each SF3 sample's byte range. |
| SFZ/SF2/EXS sample files (WAV, AIFF, FLAC, MP3) | `symphonia` bundle, `hound` | 0.5 / 3.5 | MPL-2.0 (named) / Apache-2.0 | allowed | `ph-formats` | No change. |
| Kontakt zlib-compressed sections | `miniz_oxide` | 0.8.9 (0.9.1 also present) | MIT OR Zlib OR Apache-2.0 | allowed | `png` (via `ph-ui`) | Pure Rust inflate. If T6 needs it, depend on `miniz_oxide` directly (no C, no `flate2` front end). |
| (alternative to the above) | `flate2` | 1.1.10 | MIT OR Apache-2.0 | allowed | `png` | Only if a streaming `Read` adapter is wanted; it pulls `miniz_oxide` anyway. |

## Write ourselves (no crate)

| format part | why our own | track |
|---|---|---|
| **SFZ** text (headers, opcodes, `#define`, `#include`) | A line-oriented text format; the spec is small and our parser must list unknown opcodes, which a generic crate would not. | T3 |
| **SF2/SF3** RIFF chunks (`INFO`, `sdta`, `pdta`: `phdr`/`pbag`/`pgen`/`pmod`/`inst`/`ibag`/`igen`/`imod`/`shdr`) | Fixed-size little-endian records; a few hundred lines. Existing crates (`soundfont`, `rustysynth`, MIT) were **not** read (clean room) and are not needed. | T4 |
| **EXS24** binary chunks | Undocumented by Apple; T1 writes the spec from generated/redistributable files; small fixed records. | T4 |
| **Kontakt** NKI/NKM/NKX/NKC containers, monolith index, **NCW** decoding | No suitable crate exists; NCW is bit-packed delta blocks. Encryption detection is part of our parser (refuse, never decrypt). | T6 |
| **DecentSampler** `.dspreset` XML | See below: our own minimal XML reader is preferred. | T4 |

## Open choice for T4: XML for DecentSampler

| option | crate | licence | in the tree? | weight |
|---|---|---|---|---|
| **A (preferred)** | our own minimal XML reader (elements, attributes, text, comments, entities `&amp; &lt; &gt; &quot; &apos;` and numeric references; no DTD) | ours | n/a | ~300 lines; `.dspreset` files are plain element/attribute trees |
| B | `quick-xml` | MIT | 0.41.0 in `Cargo.lock`, only as a Linux build dependency (`wayland-scanner`), not in our runtime graph | small, pure Rust, no default dependencies needed |
| C | `roxmltree` | MIT OR Apache-2.0 | no | small, read-only DOM |

All three pass `deny.toml`. T4 decides; choosing B or C adds a row to the contract log and
runs `cargo xtask notices`.

## Later milestones (not M3)

| need | crate | licence | note |
|---|---|---|---|
| Luau script host (M3b) | `mlua` with `luau` (vendors Luau, MIT) | MIT | DESIGN §8; heavy (C++ build). Not added in M3. |

## Checks run

- `cargo deny check` passes with the M3/T0 tree (no new crates).
- Licences above are from `cargo metadata` on this tree (2026-11-05).
