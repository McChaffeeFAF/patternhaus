# Haus Sampler format parity tracker

One table per import format (M3 criterion 2). Rows were seeded by M3/T1 from the format
specs in `docs/formats/`. **Implementers fill in the last four columns** when the feature
lands:

- **Supported:** imported and played as the format means it. Write the date.
- **Approximated:** imported, but played differently. Say how (e.g. "curve snapped to
  exponential").
- **Skipped:** not imported. It must still be listed in the import report (never silently
  dropped). Write "reported" once the report lists it.
- **Test:** the fixture id(s) from the spec's fixture plan (and the test name) that prove
  the row.

**M3 target** is the spec's recommendation: **must** (support in M3), **approx** (an
approximation is acceptable), **skip** (may be skipped, but reported), **data** (kept as
data in the native instrument for a later milestone, and reported). Exactly one of
Supported / Approximated / Skipped is filled per row; a **must** row needs Supported or a
written reason.

Fixture ids are defined in each spec's last section; the shared rules are in
[`docs/formats/sampler-fixtures.md`](../formats/sampler-fixtures.md).

---

## SFZ ([spec](../formats/sfz.md))

Filled by M3/T3 (2026-11-05). Fixtures and their checks: `crates/ph-sampler-formats/tests/sfz_fixtures.rs`
(`sfz_fixtures_import_render_and_round_trip`: model checks, render reference in
`tests/golden/sfz/`, native round trip renders bit-identically), lexer and preprocessor unit
tests in `src/sfz/lex.rs` and `src/sfz/opcodes.rs`, fuzzing in `tests/fuzz.rs`
(`fuzz_sfz`, `fuzz_sfz_mutated`). **Pending T2** marks a fixture whose model is checked now but
whose sound needs M3/T2's DSP (envelopes, loops, filters, LFOs, crossfades, the modulation
matrix, group effects): its reference holds the T0 engine's sound and is regenerated when T2
lands. "Model gap" means the Haus Sampler model (`ph_core::sampler`) has nothing to map the
opcode onto; the importer reports it, and a model change is needed to support it.

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| Lexer: comments, whitespace, values with spaces, CRLF | §1 | must | 2026-11-05 | | | F-SFZ-15; `lex::tests::comments_line_endings_and_spaced_values` |
| `#define` substitution (longest name first) | §2.1 | must | 2026-11-05 | | | F-SFZ-12; `lex::tests::defines_longest_first_and_in_stream_order` |
| `#include` (main-file-relative, nesting, cycle check) | §2.2 | must | 2026-11-05 (paths may not leave the main file's folder) | | | F-SFZ-12; `lex::tests::includes_are_confined_and_cycles_stop` |
| Headers `<control> <global> <master> <group> <region>` and inheritance resets | §3.1–3.2 | must | 2026-11-05 | | | F-SFZ-02 |
| `<curve>` and built-in curves 0–6 | §3.5 | must | curves parsed and interpolated; built-ins 0, 4, 5 exact | other built-ins and custom curves: nearest of linear, x², √x (reported) | | F-SFZ-14 (pending T2) |
| `<effect>` blocks | §12 | approx (reverb, delay, comp, eq, filter, phaser, disto) / data (others) | | main-bus effects on every group, `fxN` effects on the groups that send to them (mix = `effectN`), built-in defaults; every block kept in `extra["sfz.effects"]` | other types and aux buses (reported, kept as data) | F-SFZ-16 (pending T2) |
| `<midi>`, `<sample>` headers | §3.1 | skip | | | reported | F-SFZ-15 |
| Note names, `note_offset`, `octave_offset`, `sw_*_offset` | §3.3 | must | 2026-11-05 | | | F-SFZ-12; `opcodes::tests::note_names` |
| Sample paths, `default_path`, case-insensitive fallback | §3.4 | must | 2026-11-05 (case fixed per path component, reported) | | | F-SFZ-12, F-SFZ-15 |
| Built-in samples `*sine`, `*silence` / others | §3.4 | must / skip | `*sine` (looped 375 Hz table, tuned to the root), `*silence` | | other generators (reported, regions skipped) | F-SFZ-15 |
| Key and velocity mapping, `key=` expansion, channel range | §4.1 | must | 2026-11-05 (regions not on channel 1 skipped and reported) | | `lokey=-1` regions (controller-triggered only) | F-SFZ-01 |
| Triggers `attack`, `release`, `release_key`, `first`, `legato` | §4.2 | must | `attack`, `release`, `release_key` (release regions one-shot) | `first`, `legato` played as attack (model gap) | | F-SFZ-05 |
| `rt_decay` | §4.2 | must | 2026-11-05 | | `rt_dead` accepted, ignored (reported) | F-SFZ-05 |
| Round robin `seq_length` / `seq_position` | §4.3 | must | 2026-11-05 (zone positions; the group's sequential cycle) | | | F-SFZ-04 |
| Random `lorand` / `hirand` (seeded) | §4.3 | must | | the group's seeded random round robin (equal chances, never the same twice; reported) | with a sequence in the same region (reported) | F-SFZ-04 |
| Keyswitches `sw_lokey/hikey/last/lolast/hilast/down/up/previous/default/label` | §4.4 | must | `sw_last`, `sw_lolast`/`sw_hilast`, `sw_default`, `sw_label`, `sw_lokey`/`sw_hikey` (articulations) | no `sw_default`: the first articulation plays first (reported) | `sw_down`, `sw_up`, `sw_previous` (model gap, reported) | F-SFZ-06, F-SFZ-17 |
| `sw_vel` | §4.4 | skip | | | reported | `opcodes::leftover` |
| CC conditions `loccN/hiccN`, HD CCs, bend, channel aftertouch | §4.5 | must | `loccN`/`hiccN`, `lohdccN`/`hihdccN` (CC 0–127) | | bend and aftertouch conditions, extended-CC conditions (model gap, reported) | F-SFZ-07 |
| Poly aftertouch, program, bpm, timer conditions | §4.5 | skip | | | reported | `opcodes::leftover` |
| `set_ccN`, sustain opcodes | §4.5 | must | | `set_ccN`/`set_hdccN` kept in `extra["sfz.control"]` (model gap: no initial controller values) | `sustain_sw=off`, `sustain_cc`, `sustain_lo` (model gap, reported) | F-SFZ-07 |
| CC-triggered regions `on_loccN/on_hiccN` | §4.6 | must | | | regions skipped and reported (model gap: zones trigger from notes only) | `region` mapper |
| Extended CCs 128, 129, 131, 133, 135, 136 | §4.7 | must | as modulation sources: 128 bend, 129 aftertouch, 130 poly pressure, 131 velocity, 135 random | 133 (key) and 136 (bipolar random) through the nearest source (reported) | others (reported) | `cc_source` |
| `offset`, `offset_random`, `offset_onccN`, `end` | §5 | must | 2026-11-05 (`end` inclusive; `end=0`/`-1` regions skipped) | | | F-SFZ-08 (pending T2) |
| `delay`, `delay_random` | §5 | must | | `delay` added to the amp envelope's delay (reported) | `delay_random` (model gap, reported) | `region` mapper |
| `direction=reverse` | §5 | must | 2026-11-05 | | | F-SFZ-08 (pending T2) |
| `count`, sync/beats opcodes, `sample_fadeout` | §5 | approx / skip | | `count` as one-shot | sync/beats, `sample_fadeout` (reported) | |
| Loop modes, `loop_start/end`, file loop default, ARIA whole-sample rule | §5.1 | must | 2026-11-05 (`smpl` loop default; `loop_sustain` = loop exits at note-off) | loops reaching outside the played range are cut to fit (reported) | | F-SFZ-08 (pending T2) |
| `loop_type` (alternate, backward), `loop_crossfade` | §5.1 | must | `alternate` (ping-pong), `loop_crossfade` (seconds to frames) | `backward` as forward (reported) | | F-SFZ-08 (pending T2) |
| `volume`, `amplitude`, `pan`, `width`, `position`, `output` | §6 | must | `volume`, `amplitude`, per-level volumes/amplitudes (summed in dB), `pan`, `output` (0–15) | | `width`, `position` (model gap, reported) | F-SFZ-01, F-SFZ-02 |
| Velocity curve: `amp_veltrack`, `amp_velcurve_N`, `amp_random`, `amp_keytrack` | §6 | must | `amp_random`, `amp_keytrack` (+ `amp_keycenter`) as modulation routes | `amp_veltrack`, `amp_velcurve_N`: the sampler's own velocity response (reported) | | F-SFZ-03 (pending T2) |
| CC gain/pan/amplitude modulation with `_curveccN` | §6 | must | `gain_onccN`, `pan_onccN` with `_curveccN` | | `amplitude_onccN` (model gap: no linear-gain route, reported) | F-SFZ-14 (pending T2) |
| `_smoothccN`, `_stepccN` | §6 | skip | | | reported | `opcodes::leftover` |
| Crossfades key / velocity / CC with `gain` and `power` curves | §6.1 | must | velocity and CC (0–127) crossfades, both curves | overlapping ranges put in order (reported) | key crossfades (model gap, reported) | F-SFZ-03, F-SFZ-07 (pending T2) |
| Pitch: keycenter, keytrack, tune, transpose, veltrack, random, bend range | §7 | must | all (keytrack ≠ 100 and veltrack/random as routes; bend range instrument-wide) | regions with different bend ranges share the first (reported) | | F-SFZ-01 |
| `pitch_onccN` | §7 | must | 2026-11-05 (route) | | | `region` mapper |
| `ampeg_*` DAHDSR and `vel2*` | §8.1 | must | DAHDSR (group amp envelope) | | `vel2*`, `ampeg_start` (model gap, reported) | F-SFZ-09 (pending T2) |
| `ampeg_*ccN`, shape opcodes | §8.1 | skip / approx | | | reported | `opcodes::leftover` |
| `fileg_*`, `pitcheg_*` | §8.2 | must | `fileg_*` (filter envelope + `env_depth`) | `pitcheg_*` through the group's second envelope when the filter does not use it (reported) | `pitcheg_*` when `fileg_*` is set; `*_vel2depth` (reported) | F-SFZ-09 (pending T2) |
| `egN_*` flex envelopes | §8.3 | approx | | | reported (model gap: no multi-point envelope) | `opcodes::leftover` |
| `amplfo_*`, `fillfo_*`, `pitchlfo_*` | §9.1 | must | `_freq`, `_delay`, `_fade`, `_depth` (group LFO + route) | | controller/aftertouch forms (reported) | F-SFZ-10 (pending T2) |
| `lfoN_*` waves 0–7, targets pitch/volume/amplitude/pan/cutoff/resonance, `_onccN` | §9.2 | must | waves 0, 1, 3, 6, 7, 12; targets pitch, volume, pan, cutoff, resonance | pulse waves 2, 4, 5 as square (reported) | `lfoN_amplitude`, `lfoN_phase`, `_onccN` depth (model gap, reported); more than 4 LFOs per group | F-SFZ-10 (pending T2) |
| LFO waves 12–13, LFO-to-LFO, sub-oscillators | §9.2 | skip | wave 12 (sample and hold) | wave 13 and others as sine (reported) | LFO-to-LFO, sub-oscillators (reported) | F-SFZ-10 |
| Filter 1 and 2: `cutoff`, `resonance`, v1 types, tracking, random, CC | §10 | must | filter 1: cutoff, resonance, `lpf_2p`/`hpf_2p`/`bpf_2p`/`brf_2p`, `fil_keytrack` + `fil_keycenter`, `fil_veltrack`, `fil_random`, `cutoff_onccN`, `resonance_onccN` | `_1p` types at 12 dB/oct (reported) | filter 2 (model gap: one filter per group, reported) | F-SFZ-11 (pending T2) |
| v2/ARIA filter types (4p, 6p, sv, shelves) | §10 | approx | `lpf_4p`, `hpf_4p`, `_sv` types | `_6p` at 24 dB/oct (reported) | `pkf_2p`, `lsh`, `hsh`, `peq` (filter bypassed, reported) | F-SFZ-11 (pending T2) |
| `comb`, `pink`, `apf_1p` | §10 | skip | | | filter bypassed, reported | `region` mapper |
| `eqN_*` | §10 | approx | | | reported (not yet mapped to a group EQ) | `opcodes::leftover` |
| `group` / `off_by` / `off_mode` / `off_time` | §11 | must | | exclusive groups share a Haus group limited to one voice (reported); `off_mode`/`off_time` read, the engine's stealing fade applies | | F-SFZ-13 (pending T2) |
| `polyphony`, `note_polyphony` | §11 | must | `polyphony` (group voice limit) | | `note_polyphony` (model gap, reported) | F-SFZ-13 |
| Labels (`label_ccN`, `*_label`) | §13 | data | `group_label`/`master_label`/`global_label` name groups, `sw_label` names articulations; `label_ccN` kept in `extra["sfz.control"]` | | `region_label`, `md5`, `image` reported as kept, not stored | F-SFZ-02, F-SFZ-06 |
| Opcode aliases | §14 | must | 2026-11-05 | | | F-SFZ-15; `opcodes::tests::aliases` |
| Unknown opcodes listed in the report with counts | §15 | must | 2026-11-05 (report `unknown` and `import_notes`) | | | F-SFZ-15 |
| Acceptance instrument (criterion 1) | §18 | must | 2026-11-05 | | | F-SFZ-17 |
| Malformed input never panics | §15 | must | 2026-11-05 | | | `malformed_fixtures_never_panic`, `fuzz_sfz`, `fuzz_sfz_mutated` |
| Drop to create: a folder of samples auto-mapped by note names | M3 T3/T5 | must | 2026-11-05 (rule in `ph_sampler_formats::automap`) | | files without a note name (reported) | `automap::tests`, `sampler_import_maps_a_folder_of_samples_by_note_names` |

## SF2 / SF3 ([spec](../formats/sf2.md))

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| RIFF container, INFO metadata, version | §1–2 | must | | | | F-SF2-01 |
| 16-bit `smpl` | §3 | must | | | | F-SF2-01 |
| 24-bit `sm24` and its ignore rules | §3 | must | | | | F-SF2-11 |
| Hydra records and structural rejection rules | §4 | must | | | | F-SF2-12 |
| Preset selection (bank:program; bank 128 percussion) | §4.1, §7 | must | | | | F-SF2-02 |
| Global zones, generator ordering rules | §4.4 | must | | | | F-SF2-03 |
| Generator summing (instrument absolute, preset relative, range intersection) | §7 | must | | | | F-SF2-03 |
| Sample addresses and coarse offsets | §5.1 | must | | | | F-SF2-05 |
| `sampleModes` 0/1/3 | §10 | must | | | | F-SF2-05 |
| Pitch: root key, override, correction, coarse/fine, scaleTuning, keynum/velocity substitution | §5.2 | must | | | | F-SF2-06 |
| initialAttenuation with the 0.4 convention | §8.4 | approx | | | | F-SF2-04 |
| Pan | §5 | must | | | | |
| Volume envelope (dB decay/release) | §10 | must | | | | F-SF2-07 |
| Modulation envelope → pitch, cutoff | §10 | must | | | | F-SF2-07 |
| keynumTo… hold/decay | §10 | must | | | | F-SF2-07 |
| Mod LFO, vib LFO | §10 | must | | | | F-SF2-08 |
| Low-pass filter Fc, Q | §10 | must | | | | F-SF2-08 |
| Default modulators (10) | §8.4 | must | | | | F-SF2-09 |
| File modulators: sources, curves, polarity, direction, transforms | §8 | must | | | | F-SF2-09 |
| Modulator override/add hierarchy | §8.3 | must | | | | F-SF2-04, F-SF2-09 |
| Linked modulators | §8.2 | skip | | | | F-SF2-09 |
| exclusiveClass (choke) | §10 | must | | | | F-SF2-02 |
| Reverb / chorus sends | §10 | data | | | | |
| Stereo pairs joined into stereo zones | §6.2 | approx | | | | F-SF2-10 |
| ROM samples | §6.3 | skip | | | | F-SF2-12 |
| SF3 Ogg Vorbis samples, offsets, loops, padding | §9 | must | | | | F-SF3-01 |
| SF3 invalid/unsupported streams reported | §9 | must | | | | F-SF3-02 |

## DecentSampler ([spec](../formats/dspreset.md))

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| XML parse, paths relative to the preset | §1 | must | | | | F-DS-01 |
| `.dsbundle` folders, `DSLibraryInfo.xml` | §1 | must / data | | | | F-DS-14 |
| `.dslibrary` archives | §1 | approx (if zip support) | | | | |
| Inheritance; volume multiply; pan offsets; tuning sum | §3.1 | must | | | | F-DS-02 |
| Volume in linear and dB | §3.2 | must | | | | F-DS-02 |
| Sample mapping, start/end, tuning, pan, key tracking | §3.5 | must | | | | F-DS-01 |
| `delay`, `delayUnit` | §3.5 | must | | | | |
| Triggers incl. `continuous`, `releaseTriggerDecay` | §3.5 | must | | | | F-DS-04 |
| `previousNotes`, `legatoInterval` | §3.5 | must | | | | F-DS-04 |
| CC conditions and CC triggers | §3.5 | must | | | | F-DS-05 |
| Round robin (`seqMode` all four) | §3.5 | must | | | | F-DS-03 |
| Loops, crossfade modes, `loopDuringRelease` | §3.5 | must | | | | F-DS-06 |
| `ampEnvEnabled=false` (one-shot) | §3.4 | must | | | | F-DS-06 |
| AHDSR envelope and aliases | §3.5 | must | | | | F-DS-07 |
| Envelope curves | §3.5 | approx | | | | F-DS-07 |
| Silencing (`tags`, `silencedByTags`, modes, decay) | §3.5 | must | | | | F-DS-08 |
| Output targets, aux outputs | §3.5 | must | | | | F-DS-13 |
| Glide | §3.3 | must | | | | |
| Retrigger, slice stretch | §3.5 | skip | | | | |
| Oscillators | §3.6 | data | | | | F-DS-13 |
| Effects: filters, gain, convolution | §4 | must | | | | F-DS-10 |
| Effects: reverb, delay, chorus, phaser, compressor, stereo, bit crusher | §4 | approx | | | | F-DS-10 |
| Effects: pitch shift, wave folder/shaper, stutter | §4 | skip | | | | F-DS-10 |
| Group effect chains | §4 | approx | | | | F-DS-10 |
| UI knobs/menus/buttons → native controls via bindings | §5.1–5.2 | approx | | | | F-DS-11 |
| `<ui>` kept as data | §5.2 | data | | | | F-DS-11 |
| MIDI CC bindings | §5.3 | must | | | | F-DS-11 |
| Keyswitches from `<midi><note>` group-enable bindings | §5.3 | must | | | | F-DS-09 |
| Modulators: LFO, envelope, midiCC, velocity, random | §5.4 | must | | | | F-DS-12 |
| MPE modulators | §5.4 | skip | | | | |
| Tags (enable, volume, polyphony) | §5.5 | must | | | | F-DS-13 |
| Buses with effects | §5.5 | approx | | | | F-DS-10 |
| Note sequences, arpeggiator | §5.6 | data | | | | F-DS-13 |
| Limiter style | §2 | skip | | | | |

## EXS24 ([spec](../formats/exs.md)) — layout unconfirmed on real files

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| Block chain, magics, byte order, size flag | §1 | must | | | | F-EXS-01, F-EXS-02 |
| 0x40 variant and plist blocks 0x0A/0x0B | §1.3 | data | | | | F-EXS-10 |
| Instrument block counts cross-check | §2 | must | | | | F-EXS-01 |
| Zone mapping, tunes, volume, pan, pitch off, reverse, one-shot | §3 | must | | | | F-EXS-04 |
| Zone loops (direction, crossfade, play-to-end) | §3 | must | | | | F-EXS-05 |
| Zone output assignment | §3 | must | | | | |
| Groups: volume, pan, polyphony, exclusive, limits, mute | §4 | must | | | | F-EXS-06 |
| Release-trigger groups | §4 | must | | | | F-EXS-06 |
| Round robin (enable-by 2) | §4 | must | | | | F-EXS-07 |
| Keyswitch / CC / articulation enable | §4 | must / approx | | | | F-EXS-07 |
| Tempo / bend / channel enable | §4 | skip | | | | |
| Group crossfades | §4 | approx | | | | |
| Sample lookup rules | §5 | must | | | | F-EXS-09 |
| Consolidated single-file samples | §5 | must (WAV/AIFF) / skip (CAF/AAC) | | | | |
| Parameters: master volume/pan, tune, transpose, bend, polyphony, legato | §6 | must | | | | F-EXS-08 |
| Parameters: amp and filter envelopes | §6 | must | | | | F-EXS-08 |
| Parameters: filter 1 | §6 | approx | | | | F-EXS-08 |
| Parameters: filter 2, LFOs, glide, velocity curve, random pitch, crossfade | §6 | approx | | | | |
| Modulation matrix | §6 | data | | | | |

## Kontakt

Not part of this repository. By the owner's decision (2026-11-05), Kontakt import is an optional component kept outside the public project.

