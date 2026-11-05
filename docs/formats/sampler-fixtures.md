# Sampler import fixtures: plan

**Status:** M3/T1 (2026-11-05). The per-format fixture lists live at the end of each spec;
this page holds the rules they share and an index. M3 criterion 2 requires that every
importer has generated fixtures covering its supported opcodes or chunks, each rendering to
a reference, and that import → save as native → reopen renders identically.

## Rules

1. **Generated only.** Every fixture is written by test code (a small writer per format in
   `ph-sampler-formats`, e.g. `build::Sf2Builder`, `build::ExsBuilder`,
   `build::KontaktBuilder`, `build::NcwWriter`, like M2's `FlpBuilder`) or by
   `tools/soundgen`. Nothing is copied from a library, a converter's test data or the
   internet. No NI, Apple or third-party file enters the repo.
2. **Audio.** Sample content comes from soundgen voices. Use tones whose pitch or a short
   DC "ID level" at the start identifies which zone played, so a render can be checked
   automatically (which zone, at which pitch, how loud) as well as against its reference.
   Keep samples short (≤ 0.5 s) and mono unless the fixture tests stereo.
3. **Text formats** (SFZ, DecentSampler) are written as strings in the test, with the
   samples written beside them in a temp folder. **Binary formats** (SF2/SF3, EXS, Kontakt,
   NCW) are built byte by byte from the spec, in both byte orders where the format has two.
4. **Committed bytes.** Prefer generating at test time. If a fixture must be committed
   (e.g. a tiny Ogg Vorbis stream for SF3 when no encoder is available), commit it with a
   provenance entry: the command that made it and the soundgen voice used.
5. **References.** Each fixture renders a fixed MIDI phrase (defined next to the fixture)
   to a reference WAV at 48 kHz; references follow the M1/M2 golden-render conventions.
6. **Round trip.** Each fixture is also imported, saved as a native instrument
   (`docs/formats/haus-instrument.md`, T3), reopened and rendered; the two renders are
   bit-identical.
7. **Fuzz seeds.** The "malformed" fixtures of each format are the fuzzing corpus seeds.
8. **Unconfirmed layouts.** EXS and Kontakt fixtures test agreement with our spec only.
   Compatibility with real files is checked against the owner's libraries (Kontakt: M3
   criterion 3; EXS: the controlled files in `exs.md` §9), never by committing those files.
9. **Protection fixtures** (Kontakt F-NKI-12) contain synthetic markers only (a chunk with
   a placeholder product id, a zero-filled encryption chunk, random bytes, an empty
   `.nicnt` file). They never contain real protected data.

## Index

| Format | Fixtures | Defined in |
|---|---|---|
| SFZ | F-SFZ-01 … F-SFZ-17 | [`sfz.md` §18](sfz.md#18-fixture-plan-generated-never-copied) |
| SF2 / SF3 | F-SF2-01 … F-SF2-12, F-SF3-01, F-SF3-02 | [`sf2.md` §12](sf2.md#12-fixture-plan-generated-never-copied) |
| DecentSampler | F-DS-01 … F-DS-14 | [`dspreset.md` §8](dspreset.md#8-fixture-plan-generated-never-copied) |
| EXS24 | F-EXS-01 … F-EXS-10 | [`exs.md` §10](exs.md#10-fixture-plan-generated-never-copied) |
| Kontakt, NCW | F-NKI-01 … F-NKI-13, F-NCW-01 | [`nki.md` §12](nki.md#12-fixture-plan-generated-never-copied) |

The parity tracker [`docs/parity/haus-sampler.md`](../parity/haus-sampler.md) maps each
feature row to the fixtures that prove it.

## Acceptance-test instruments (criterion 1)

- **F-SFZ-17:** velocity layers, round robins, release triggers and two keyswitched
  articulations.
- **F-SF2-02:** a drum kit preset in bank 128 with a hi-hat choke.
