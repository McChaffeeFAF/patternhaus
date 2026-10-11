# Patternhaus factory assets

Everything in this folder was generated or written by the Patternhaus project and is
dedicated to the public domain under **CC0-1.0**, except [`fonts/`](fonts/README.md): the UI
typefaces, third-party and under the SIL Open Font License 1.1. You can use these sounds and presets in
any music, commercial or not, without credit. Every file has an entry in
[`PROVENANCE.toml`](PROVENANCE.toml); CI rejects any file without one.

Nothing here is downloaded, sampled or resampled from other content. The drum sounds are
synthesised by [`tools/soundgen`](../tools/soundgen) (swept-sine kicks, membrane-mode and
noise snares, square-cluster and plate-mode metals). The tool is deterministic: the same
recipe always writes byte-identical files, and `cargo run -p soundgen -- check` (run in CI)
fails if anything here differs from a fresh render. To change a sound, edit its recipe in
`tools/soundgen/src/recipes.rs` and run `cargo run -p soundgen -- render`.

## Samples

Mono, 48 kHz, 24-bit WAV, peak-normalised to -1 dBFS. Refer to a sample as
`factory:<path>`, where `<path>` is relative to `samples/` without the `.wav` extension, for
example `factory:drums/haus808/kick` is `samples/drums/haus808/kick.wav`.

Each kit has a kick, snare, rim, clap, closed/open/pedal hats, low/mid/high toms, a crash and
one percussion sound.

| Kit | Character | Percussion | Extras |
|---|---|---|---|
| `drums/haus808` | classic analog drum machine: long sine kick, two-oscillator snare, square-cluster hats | `cowbell` | `sub_c1`, `sub_c2`, `sub_c3`: long 808-style sub hits tuned to C1, C2 and C3 |
| `drums/haus909` | punchier and noisier: harder kick, bright snare, noisy hats | `shaker` | |
| `drums/acoustic-ish` | physical models: membrane-mode snare and toms, plate-mode hats and crash | `clave` | |

Sound names: `kick`, `snare`, `rim`, `clap`, `hat_closed`, `hat_open`, `hat_pedal`,
`tom_low`, `tom_mid`, `tom_high`, `crash`.

## Haus Synth presets

`presets/synth/<category>/<name>.json` maps Haus Synth parameter ids to values. They are
compiled into `ph-instruments`; an instrument names one as `bass/sub` and any `params` in
the project override it.

| Category | Presets |
|---|---|
| `bass` | `sub`, `reese`, `acid`, `pluck`, `wobble`, `growl`, `mono-saw` |
| `lead` | `supersaw`, `square`, `saw-mono`, `sine-soft`, `pwm` |
| `pad` | `warm`, `glass`, `strings`, `choir-ish`, `dark`, `sweep` |
| `pluck` | `bell`, `pizz`, `harp-ish`, `marimba`, `kalimba` |
| `keys` | `epiano-ish`, `organ`, `clav`, `piano-ish` |
| `fx` | `riser`, `sweep-down`, `noise-hit`, `laser`, `zap` |

Every preset is tested to render a C3 at full velocity without clipping.
