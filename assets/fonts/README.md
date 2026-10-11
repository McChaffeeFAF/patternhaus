# Bundled fonts

The UI typefaces (DESIGN section 9). They are third-party, not CC0: each is licensed under the
**SIL Open Font License 1.1**, whose text sits next to the fonts (`OFL-*.txt`) and is repeated
in `THIRD_PARTY_NOTICES.md`. They are used unmodified.

| File | Family | Used for |
|---|---|---|
| `BarlowCondensed-Medium.ttf`, `BarlowCondensed-SemiBold.ttf` | Barlow Condensed | labels, menus, tabs, section names, buttons (uppercased at render time) |
| `Barlow-Regular.ttf`, `Barlow-Medium.ttf` | Barlow | content the user typed, hints |
| `ShareTechMono-Regular.ttf` | Share Tech Mono | readouts: tempo, bar/beat/tick, dB, timings |

Source: the official Google Fonts repository, `github.com/google/fonts`, folders `ofl/barlow`,
`ofl/barlowcondensed` and `ofl/sharetechmono`, downloaded 2026-10-10 at commit
`bd8f81ddb5c74d5c8897b36ad88b440266245103`. Barlow is by Jeremy Tribby; Share Tech Mono by Carrois Type Design.
The per-file entries are in `../PROVENANCE.toml`, which `soundgen` writes (so the table is
reproducible); to add a font, add it to `FONTS` in `tools/soundgen/src/lib.rs`.
