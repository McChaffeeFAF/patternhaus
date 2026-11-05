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
(`fuzz_sfz`, `fuzz_sfz_mutated`). Fixtures F-SFZ-03, 07, 08, 09, 10, 11, 13, 14 and 16 were
*pending T2* (their sound needed M3/T2's DSP). Since M3/T2 (2026-11-05) their references are
regenerated with the real engine and each also has a **sound check** on the render
(`tests/sfz_sound/mod.rs`, run by the same test): the layer mix each velocity hears through its
crossfade gains (03), the CC layer and crossfade at controller 0 (07), loops sounding past the
sample's end while held, the sustain loop's exit, one-shot and reverse (08), the DAHDSR stages
and the pitch envelope's bend (09), the amplitude, square-volume and pitch LFOs (10), low-,
high-, band-pass and notch on a 220 Hz tone (11), the hi-hat choke and the voice-limited group
(13), unity CC curves at controller 0 (14), and the group chains' reverb and delay tails (16).
"Model gap" means the Haus Sampler model (`ph_core::sampler`) has nothing to map the
opcode onto; the importer reports it, and a model change is needed to support it.

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| Lexer: comments, whitespace, values with spaces, CRLF | §1 | must | 2026-11-05 | | | F-SFZ-15; `lex::tests::comments_line_endings_and_spaced_values` |
| `#define` substitution (longest name first) | §2.1 | must | 2026-11-05 | | | F-SFZ-12; `lex::tests::defines_longest_first_and_in_stream_order` |
| `#include` (main-file-relative, nesting, cycle check) | §2.2 | must | 2026-11-05 (paths may not leave the main file's folder) | | | F-SFZ-12; `lex::tests::includes_are_confined_and_cycles_stop` |
| Headers `<control> <global> <master> <group> <region>` and inheritance resets | §3.1–3.2 | must | 2026-11-05 | | | F-SFZ-02 |
| `<curve>` and built-in curves 0–6 | §3.5 | must | curves parsed and interpolated; built-ins 0, 4, 5 exact | other built-ins and custom curves: nearest of linear, x², √x (reported) | | F-SFZ-14 (sound checked at controller 0 only) |
| `<effect>` blocks | §12 | approx (reverb, delay, comp, eq, filter, phaser, disto) / data (others) | | main-bus effects on every group, `fxN` effects on the groups that send to them (mix = `effectN`), built-in defaults; every block kept in `extra["sfz.effects"]` | other types and aux buses (reported, kept as data) | F-SFZ-16 (sound checked) |
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
| `offset`, `offset_random`, `offset_onccN`, `end` | §5 | must | 2026-11-05 (`end` inclusive; `end=0`/`-1` regions skipped) | | | F-SFZ-08 (sound checked) |
| `delay`, `delay_random` | §5 | must | | `delay` added to the amp envelope's delay (reported) | `delay_random` (model gap, reported) | `region` mapper |
| `direction=reverse` | §5 | must | 2026-11-05 | | | F-SFZ-08 (sound checked) |
| `count`, sync/beats opcodes, `sample_fadeout` | §5 | approx / skip | | `count` as one-shot | sync/beats, `sample_fadeout` (reported) | |
| Loop modes, `loop_start/end`, file loop default, ARIA whole-sample rule | §5.1 | must | 2026-11-05 (`smpl` loop default; `loop_sustain` = loop exits at note-off) | loops reaching outside the played range are cut to fit (reported) | | F-SFZ-08 (sound checked) |
| `loop_type` (alternate, backward), `loop_crossfade` | §5.1 | must | `alternate` (ping-pong), `loop_crossfade` (seconds to frames) | `backward` as forward (reported) | | F-SFZ-08 (sound checked) |
| `volume`, `amplitude`, `pan`, `width`, `position`, `output` | §6 | must | `volume`, `amplitude`, per-level volumes/amplitudes (summed in dB), `pan`, `output` (0–15) | | `width`, `position` (model gap, reported) | F-SFZ-01, F-SFZ-02 |
| Velocity curve: `amp_veltrack`, `amp_velcurve_N`, `amp_random`, `amp_keytrack` | §6 | must | `amp_random`, `amp_keytrack` (+ `amp_keycenter`) as modulation routes | `amp_veltrack`, `amp_velcurve_N`: the sampler's own velocity response (reported) | | F-SFZ-03 (sound checked) |
| CC gain/pan/amplitude modulation with `_curveccN` | §6 | must | `gain_onccN`, `pan_onccN` with `_curveccN` | | `amplitude_onccN` (model gap: no linear-gain route, reported) | F-SFZ-14 (sound checked at controller 0 only) |
| `_smoothccN`, `_stepccN` | §6 | skip | | | reported | `opcodes::leftover` |
| Crossfades key / velocity / CC with `gain` and `power` curves | §6.1 | must | velocity and CC (0–127) crossfades, both curves | overlapping ranges put in order (reported) | key crossfades (model gap, reported) | F-SFZ-03, F-SFZ-07 (sound checked; 07 at controller 0: `set_cc1` is a model gap) |
| Pitch: keycenter, keytrack, tune, transpose, veltrack, random, bend range | §7 | must | all (keytrack ≠ 100 and veltrack/random as routes; bend range instrument-wide) | regions with different bend ranges share the first (reported) | | F-SFZ-01 |
| `pitch_onccN` | §7 | must | 2026-11-05 (route) | | | `region` mapper |
| `ampeg_*` DAHDSR and `vel2*` | §8.1 | must | DAHDSR (group amp envelope) | | `vel2*`, `ampeg_start` (model gap, reported) | F-SFZ-09 (sound checked) |
| `ampeg_*ccN`, shape opcodes | §8.1 | skip / approx | | | reported | `opcodes::leftover` |
| `fileg_*`, `pitcheg_*` | §8.2 | must | `fileg_*` (filter envelope + `env_depth`) | `pitcheg_*` through the group's second envelope when the filter does not use it (reported) | `pitcheg_*` when `fileg_*` is set; `*_vel2depth` (reported) | F-SFZ-09 (sound checked) |
| `egN_*` flex envelopes | §8.3 | approx | | | reported (model gap: no multi-point envelope) | `opcodes::leftover` |
| `amplfo_*`, `fillfo_*`, `pitchlfo_*` | §9.1 | must | `_freq`, `_delay`, `_fade`, `_depth` (group LFO + route) | | controller/aftertouch forms (reported) | F-SFZ-10 (sound checked) |
| `lfoN_*` waves 0–7, targets pitch/volume/amplitude/pan/cutoff/resonance, `_onccN` | §9.2 | must | waves 0, 1, 3, 6, 7, 12; targets pitch, volume, pan, cutoff, resonance | pulse waves 2, 4, 5 as square (reported) | `lfoN_amplitude`, `lfoN_phase`, `_onccN` depth (model gap, reported); more than 4 LFOs per group | F-SFZ-10 (sound checked) |
| LFO waves 12–13, LFO-to-LFO, sub-oscillators | §9.2 | skip | wave 12 (sample and hold) | wave 13 and others as sine (reported) | LFO-to-LFO, sub-oscillators (reported) | F-SFZ-10 |
| Filter 1 and 2: `cutoff`, `resonance`, v1 types, tracking, random, CC | §10 | must | filter 1: cutoff, resonance, `lpf_2p`/`hpf_2p`/`bpf_2p`/`brf_2p`, `fil_keytrack` + `fil_keycenter`, `fil_veltrack`, `fil_random`, `cutoff_onccN`, `resonance_onccN` | `_1p` types at 12 dB/oct (reported) | filter 2 (model gap: one filter per group, reported) | F-SFZ-11 (sound checked) |
| v2/ARIA filter types (4p, 6p, sv, shelves) | §10 | approx | `lpf_4p`, `hpf_4p`, `_sv` types | `_6p` at 24 dB/oct (reported) | `pkf_2p`, `lsh`, `hsh`, `peq` (filter bypassed, reported) | F-SFZ-11 (sound checked) |
| `comb`, `pink`, `apf_1p` | §10 | skip | | | filter bypassed, reported | `region` mapper |
| `eqN_*` | §10 | approx | | | reported (not yet mapped to a group EQ) | `opcodes::leftover` |
| `group` / `off_by` / `off_mode` / `off_time` | §11 | must | | exclusive groups share a Haus group limited to one voice (reported); `off_mode`/`off_time` read, the engine's stealing fade applies | | F-SFZ-13 (sound checked) |
| `polyphony`, `note_polyphony` | §11 | must | `polyphony` (group voice limit) | | `note_polyphony` (model gap, reported) | F-SFZ-13 |
| Labels (`label_ccN`, `*_label`) | §13 | data | `group_label`/`master_label`/`global_label` name groups, `sw_label` names articulations; `label_ccN` kept in `extra["sfz.control"]` | | `region_label`, `md5`, `image` reported as kept, not stored | F-SFZ-02, F-SFZ-06 |
| Opcode aliases | §14 | must | 2026-11-05 | | | F-SFZ-15; `opcodes::tests::aliases` |
| Unknown opcodes listed in the report with counts | §15 | must | 2026-11-05 (report `unknown` and `import_notes`) | | | F-SFZ-15 |
| Acceptance instrument (criterion 1) | §18 | must | 2026-11-05 | | | F-SFZ-17 |
| Malformed input never panics | §15 | must | 2026-11-05 | | | `malformed_fixtures_never_panic`, `fuzz_sfz`, `fuzz_sfz_mutated` |
| Drop to create: a folder of samples auto-mapped by note names | M3 T3/T5 | must | 2026-11-05 (rule in `ph_sampler_formats::automap`) | | files without a note name (reported) | `automap::tests`, `sampler_import_maps_a_folder_of_samples_by_note_names` |

## SF2 / SF3 ([spec](../formats/sf2.md))

Filled by M3/T4 (2026-11-05). Fixtures: `crates/ph-sampler-formats/tests/sf2_fixtures.rs`
(`sf2_fixtures_import_render_and_round_trip`: model checks, render reference in
`tests/golden/sf2/`, native round trip renders bit-identically; files built with
`build::Sf2Builder`, SF3 streams with the fixture Vorbis writer `build::vorbis`), unit tests in
`src/sf2/gens.rs`, fuzzing `fuzz_sf2` and `fuzz_sf2_mutated` (seeds: `tests/sf2_seeds`,
F-SF2-12). F-SF2-05, 07 and 08 were *pending T2*. Since the M3/T2 + T4 merge (2026-11-05) every
reference holds the real engine's sound, and those three also have a **sound check** on the render
(`tests/formats_sound/mod.rs`, run by the same test): sampleModes 0 and 2 stop at the sample's
end while modes 1 and 3 (and a loop ending on the last frame) sound past it while held (05); the
volume envelope's silent delay, rising attack, decay after the hold and falling release, the
modulation envelope's pitch bend and a zero attack (07); the mod LFO's level swing against an
unmodulated key (08).

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| RIFF container, INFO metadata, version | §1–2 | must | 2026-11-05 (INAM, ICOP, IENG, ICMT, ICRD, ISFT, IPRD kept in `extra["sf2.info.*"]`; chunks located by id; unknown chunks listed) | an SF3 `smpl`/`sdta` without its pad byte is accepted (reported) | | F-SF2-01, F-SF3-01 |
| 16-bit `smpl` | §3 | must | 2026-11-05 | | | F-SF2-01 |
| 24-bit `sm24` and its ignore rules | §3 | must | 2026-11-05 (ignored before 2.04, without `smpl`, or with the wrong size; reported) | | | F-SF2-11 |
| Hydra records and structural rejection rules | §4 | must | 2026-11-05 (record sizes, monotonic bags, terminal records, missing chunks, indices past the terminal: refused) | | | F-SF2-12; `malformed_files_are_refused_never_panic` |
| Preset selection (bank:program; bank 128 percussion) | §4.1, §7 | must | 2026-11-05 (default first by bank then program; `preset` = `bank:program` or name; every preset listed in the report; duplicates: the first is active, reported) | | | F-SF2-02; `presets_are_listed_and_chosen_by_bank_program_or_name`, `sampler_import_collects_an_sf2_preset_and_lists_the_others` |
| Global zones, generator ordering rules | §4.4 | must | 2026-11-05 (keyRange/velRange position, terminal generator, zones without one, instrument-only generators in presets; each reported) | | | F-SF2-03, F-SF2-12 |
| Generator summing (instrument absolute, preset relative, range intersection) | §7 | must | 2026-11-05 | | | F-SF2-03; `gens::tests::instrument_absolute_preset_relative` |
| Sample addresses and coarse offsets | §5.1 | must | 2026-11-05 (clamped to the sample; loops cut to the played range) | | | F-SF2-05 |
| `sampleModes` 0/1/3 | §10 | must | 2026-11-05 (1 = loop through release, 3 = loop exits at note-off) | | | F-SF2-05 (sound checked) |
| Pitch: root key, override, correction, coarse/fine, scaleTuning, keynum/velocity substitution | §5.2 | must | root, overridingRootKey, correction, coarse/fine, scaleTuning (Key → pitch route), keynum (fixed pitch) | velocity substitution: the played velocity still sets the level (model gap: no fixed velocity; reported) | | F-SF2-06 |
| initialAttenuation with the 0.4 convention | §8.4 | approx | | generators at 0.4 ×, modulators at full value (spec [PH]) | | F-SF2-04 |
| Pan | §5 | must | 2026-11-05 | | | F-SF2-03 |
| Volume envelope (dB decay/release) | §10 | must | DAHDSR times and sustain level | decay/release curve: the sampler's envelope shape (T2); times over 60 s cut (reported) | | F-SF2-07 (sound checked) |
| Modulation envelope → pitch, cutoff | §10 | must | 2026-11-05 (group filter envelope + `env_depth`; envelope 1 → pitch route) | | | F-SF2-07 (sound checked: pitch), F-SF2-08 (cutoff: the model only) |
| keynumTo… hold/decay | §10 | must | | taken at the middle key of each zone (groups split by value; reported) | | F-SF2-07 |
| Mod LFO, vib LFO | §10 | must | 2026-11-05 (triangle LFOs, rate and delay; routes to pitch, cutoff, volume) | | | F-SF2-08 (sound checked) |
| Low-pass filter Fc, Q | §10 | must | 2026-11-05 (12 dB/oct low-pass; off when open, unresonant and unmodulated) | | | F-SF2-08 (the model only: the fixture tone sits where the filter barely acts) |
| Default modulators (10) | §8.4 | must | #2 velocity → cutoff (when the filter is on), #10 pitch wheel (bend range) | #1 velocity → volume: the sampler's own velocity response (reported) | #3/#4 vibrato depth by pressure / mod wheel (model gap: no controller-scaled LFO depth), #5–#7 CC7/CC10/CC11 (left to the channel), #8/#9 sends (all reported) | F-SF2-01, F-SF2-04, F-SF2-09 |
| File modulators: sources, curves, polarity, direction, transforms | §8 | must | sources velocity, key, CC, channel and poly pressure, pitch wheel; linear and switch curves; direction and polarity (exact offsets + routes); absolute value on unipolar sources; destinations pitch, coarse/fine tune, attenuation, pan, cutoff, resonance | concave/convex curves as the sampler's √x / x²; non-linear curves on key or pitch wheel as linear (reported) | amount sources, absolute value on bipolar sources, LFO/envelope-depth and envelope-time destinations (model gap), pitch-wheel sensitivity as a source (reported) | F-SF2-09 |
| Modulator override/add hierarchy | §8.3 | must | 2026-11-05 | | | F-SF2-04, F-SF2-09; `gens::tests::modulator_hierarchy` |
| Linked modulators | §8.2 | skip | | | reported | F-SF2-09 |
| exclusiveClass (choke) | §10 | must | 2026-11-05 (one Haus group per class, one voice: a new note in the class steals the old; class kept in `extra`) | | | F-SF2-02 |
| Reverb / chorus sends | §10 | data | kept in the group's `extra` (`sf2.reverb_send`, `sf2.chorus_send`), reported | | | `sf2::map` |
| Stereo pairs joined into stereo zones | §6.2 | approx | | mutually linked left/right zones with the same ranges join into one stereo zone (pitch, loop, level from the right; pan the mean); SF3 also by L/R names (reported) | unmatched linked samples play as mono (reported) | F-SF2-10 |
| ROM samples | §6.3 | skip | | | zones skipped, reported ("needs sound ROM"; a missing `irom` reported as defective) | F-SF2-12; `a_rom_sample_without_irom_is_skipped_and_reported` |
| SF3 Ogg Vorbis samples, offsets, loops, padding | §9 | must | 2026-11-05 (byte offsets, inclusive end, loops relative to the decoded sample, PCM first, odd `smpl`) | | | F-SF3-01 |
| SF3 invalid/unsupported streams reported | §9 | must | 2026-11-05 (stereo, unknown and Opus streams, offsets past the data: zones skipped, reported) | | | F-SF3-02 |
| Samples written into the project | M3/T4 brief | must | 2026-11-05 (float WAV in `instruments/<name>/samples/`, always collected) | | | `sampler_import_collects_an_sf2_preset_and_lists_the_others` |
| Malformed input never panics | §4 | must | 2026-11-05 | | | `fuzz_sf2`, `fuzz_sf2_mutated` |

## DecentSampler ([spec](../formats/dspreset.md))

Filled by M3/T4 (2026-11-05). Fixtures: `crates/ph-sampler-formats/tests/ds_fixtures.rs`
(presets as strings beside tagged-tone samples; references in `tests/golden/dspreset/`;
native round trip), the XML reader's tests in `src/dspreset/xml.rs`, fuzzing `fuzz_dspreset`
and `fuzz_dspreset_mutated` (seeds: `tests/ds_seeds`). F-DS-06, 07, 08, 10 and 12 were *pending
T2*; since the merge they have sound checks (`tests/formats_sound/mod.rs`): the marker, explicit
and linear loops sound past the sample while held, loop-off and plain samples stop, the one-shot
outlives its note-off (06); the inherited 0.5 s attack and decay, the Long group's silent delay and
fast attack (07); the closed hat silences the open one (08); delay and reverb tails after each dry
sample (10); the pitch LFO, velocity → pitch and the envelope's pan (12). "Model gap" =
the Haus Sampler model has nothing to map onto; the importer reports it.

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| XML parse, paths relative to the preset | §1 | must | 2026-11-05 (our own reader, option A; `/` and `\`, case fixed per component and reported; not-well-formed files refused) | | | F-DS-01, F-DS-13; `xml::tests` |
| `.dsbundle` folders, `DSLibraryInfo.xml` | §1 | must / data | 2026-11-05 (first preset of the menu, else by name; every preset listed; library kept in `extra`) | | | F-DS-14 |
| `.dslibrary` archives | §1 | approx (if zip support) | | | refused with a message (no zip reader in the tree) | F-DS-14 |
| Inheritance; volume multiply; pan offsets; tuning sum | §3.1 | must | 2026-11-05 | | | F-DS-02 |
| Volume in linear and dB | §3.2 | must | 2026-11-05 | | | F-DS-02 |
| Sample mapping, start/end, tuning, pan, key tracking | §3.5 | must | 2026-11-05 (`end` inclusive; key tracking as a Key → pitch route) | tunings are applied in full under reduced key tracking (SPEC-GAP, reported) | | F-DS-01 |
| `delay`, `delayUnit` | §3.5 | must | | played as the amp envelope's delay; beats at 120 BPM (reported) | | `dspreset::import_bytes` |
| Triggers incl. `continuous`, `releaseTriggerDecay` | §3.5 | must | `attack`, `release`, `continuous`, dB decay | linear decay as a gain factor per second; `first`/`legato` play on every note (model gap, reported) | | F-DS-04 |
| `previousNotes`, `legatoInterval` | §3.5 | must | | | model gap, reported (the sample plays without the condition) | F-DS-04 |
| CC conditions and CC triggers | §3.5 | must | `loCCN`/`hiCCN` (both ends set) | | `onLoCCN`/`onHiCCN` CC-triggered samples (model gap, reported) | F-DS-05 |
| Round robin (`seqMode` all four) | §3.5 | must | `always`, `round_robin`, `random` (positions from `seqPosition`; length kept) | `true_random` as random (never twice in a row; reported) | | F-DS-03 |
| Loops, crossfade modes, `loopDuringRelease` | §3.5 | must | file markers, explicit points (inclusive end), crossfade, `loopDuringRelease=false` (loop exits) | linear crossfade mode with the sampler's curve (reported) | | F-DS-06 (sound checked) |
| `ampEnvEnabled=false` (one-shot) | §3.4 | must | 2026-11-05 | | | F-DS-06 |
| AHDSR envelope and aliases | §3.5 | must | 2026-11-05 (the `ampEnv…` spelling wins) | | | F-DS-07 (sound checked) |
| Envelope curves | §3.5 | approx | | the sampler's own envelope shape (reported) | | F-DS-07 |
| Silencing (`tags`, `silencedByTags`, modes, decay) | §3.5 | must | | a silenced sample and the samples carrying the silencing tags share one one-voice group (they silence each other); modes and decay use the stealing fade (reported) | | F-DS-08 (sound checked) |
| Output targets, aux outputs | §3.5 | must | `MAIN_OUTPUT`, `NO_OUTPUT`, `AUX_STEREO_OUTPUT_n` (group output), `BUS_n`, `output1Volume` | | outputs 2–8 (reported) | F-DS-10, F-DS-13 |
| Glide | §3.3 | must | | `glideTime` makes the group mono (`always`) or legato (`legato`): the sampler glides only there (reported) | | `dspreset::import_bytes` |
| Retrigger, slice stretch | §3.5 | skip | | | reported; the sample plays normally | `dspreset::import_bytes` |
| Oscillators | §3.6 | data | kept in `extra["dspreset.group<n>.oscillators"]`, reported | | | F-DS-13 |
| Effects: filters, gain, convolution | §4 | must | low/high/band-pass and notch → `filter`; `gain` → group or instrument level; a high- or band-pass without `frequency` keeps the guide's 22000 Hz (passes almost nothing; reported) | `lowpass_1pl`/`_4pl` at 12 dB/oct; convolution with the built-in generated hall (the IR file kept as data; reported) | | F-DS-10 (sound checked: the chain sounds, with its tails) |
| Effects: reverb, delay, chorus, phaser, compressor, stereo, bit crusher | §4 | approx | | built-in `reverb`, `delay` (free times, musical time at 120 BPM), `chorus`, `phaser`, `compressor`, `stereo`, `bitcrush`; peak and shelves → `eq` band 2; reverb and delay `wetLevel` as a send level over the full dry signal (slot mix `w/(1+w)` + an `eq` make-up slot) | | F-DS-10 (sound checked) |
| Effects: pitch shift, wave folder/shaper, stutter | §4 | skip | | | reported, kept as data | F-DS-10 |
| Group effect chains | §4 | approx | group chains | the instrument chain runs in every group's insert chain (reported) | | F-DS-10 |
| UI knobs/menus/buttons → native controls via bindings | §5.1–5.2 | approx | | | model gap: no instrument controls yet; the `<ui>` is kept for M3b and reported | F-DS-11 |
| `<ui>` kept as data | §5.2 | data | `extra["dspreset.ui"]` (keyboard colours included) | | | F-DS-09, F-DS-11 |
| MIDI CC bindings | §5.3 | must | | `<cc>` bindings to `AMP_VOLUME`, `PAN`, tunings and `FX_FILTER_FREQUENCY` become CC routes added to the stored value (depth from the binding's range; reported) | other parameters kept as data (reported) | F-DS-11 |
| Keyswitches from `<midi><note>` group-enable bindings | §5.3 | must | 2026-11-05 (articulations; default = the file's enabled set; groups no keyswitch names play always) | keys without `swallowNotes` are silent keyswitches (reported) | other `<note>` bindings kept as data (reported) | F-DS-09 |
| Modulators: LFO, envelope, midiCC, velocity, random | §5.4 | must | | group LFOs, the envelope as the group's second envelope, CC, velocity and random sources routed to pitch, pan, volume and cutoff; depth from the binding's range × `modAmount`; global LFOs run per voice (reported) | other targets kept as data | F-DS-12 (sound checked) |
| MPE modulators | §5.4 | skip | | | reported | F-DS-12 |
| Tags (enable, volume, polyphony) | §5.5 | must | enable (disabled tags' samples skipped), polyphony (group voice limit) | volume applied to the samples' levels (not a control yet; reported) | | F-DS-13 |
| Buses with effects | §5.5 | approx | | a bus's effects and volume run in each feeding group's chain (reported) | | F-DS-10 |
| Note sequences, arpeggiator | §5.6 | data | kept in `extra`, reported | | | F-DS-13 |
| Limiter style | §2 | skip | | | reported | `dspreset::import_bytes` |
| Unknown elements and attributes reported | §1 | must | 2026-11-05 | | | F-DS-13 |
| Malformed input never panics | §1 | must | 2026-11-05 | | | `malformed_presets_are_refused_never_panic`, `fuzz_dspreset`, `fuzz_dspreset_mutated` |

## EXS24 ([spec](../formats/exs.md)) — layout unconfirmed on real files

Filled by M3/T4 (2026-11-05). Fixtures: `crates/ph-sampler-formats/tests/exs_fixtures.rs`
(files built with `build::ExsBuilder` in both byte orders, samples beside them; references in
`tests/golden/exs/`; native round trip), fuzzing `fuzz_exs` and `fuzz_exs_mutated` (seeds:
`tests/exs_seeds`, F-EXS-10). These prove agreement with our spec only; real Logic files (the
controlled files of exs.md §9) are still needed. F-EXS-05 and 08 were *pending T2*; since the
merge they have sound checks (`tests/formats_sound/mod.rs`): every loop kind sounds past the
sample while held and a zone with its loop off stops (05); transpose and fine tune set the pitch,
and the attack rises (08).

| Feature | Spec | M3 target | Supported | Approximated | Skipped | Test |
|---|---|---|---|---|---|---|
| Block chain, magics, byte order, size flag | §1 | must | 2026-11-05 (byte order from the magic; mixed magics, sizes past the end refused; size flag cleared and reported; big-endian signatures in all three seen forms) | | | F-EXS-01, F-EXS-02, F-EXS-10 |
| 0x40 variant and plist blocks 0x0A/0x0B | §1.3 | data | 0x0A/0x0B kept as hex in `extra["exs.block.*"]`; the 0x40 flag read as standard (reported) | | types 5, 7, 8 skipped, others listed as unknown | F-EXS-10 |
| Instrument block counts cross-check | §2 | must | 2026-11-05 (mismatches reported) | | | F-EXS-01 |
| Zone mapping, tunes, volume, pan, pitch off, reverse, one-shot | §3 | must | 2026-11-05 (option bits per the two-source reading; pitch off as a fixed-pitch route; object-id sample fallback) | pan: ±50 scale unless a value beyond ±50 shows the −64…63 one (disputed; reported) | | F-EXS-04, F-EXS-10 |
| Zone loops (direction, crossfade, play-to-end) | §3 | must | forward, ping-pong, crossfade ms → frames, play-to-end (loop exits), loop off with points | backward loops play forward; linear crossfades use the sampler's curve (reported) | loop tune (reported) | F-EXS-05 (sound checked) |
| Zone output assignment | §3 | must | 2026-11-05 (option bit 6 → group output) | | | F-EXS-04 |
| Groups: volume, pan, polyphony, exclusive, limits, mute | §4 | must | volume, pan, polyphony, velocity/key limits (zones clipped, zones outside dropped) | exclusive groups share one one-voice group; mute as −144 dB (reported) | fixed sample select, release-trigger time (reported) | F-EXS-06 |
| Release-trigger groups | §4 | must | 2026-11-05 | release-trigger decay at 3 dB/s (reported) | | F-EXS-06 |
| Round robin (enable-by 2) | §4 | must | | the [PH] reading: groups sorted by stored position, one cycle in one Haus group (encoding unconfirmed; reported) | | F-EXS-07 |
| Keyswitch / CC / articulation enable | §4 | must / approx | enable by note (articulations, `sw_last`-style), by control (CC condition) | | articulation ids kept as data, the group plays in every articulation (reported) | F-EXS-07 |
| Tempo / bend / channel enable | §4 | skip | | | kept as data, the group plays always (reported) | `exs::import_bytes` |
| Group crossfades | §4 | approx | | | kept as data, not played (encodings unknown; reported) | `exs::import_bytes` |
| Sample lookup rules | §5 | must | stored path (folder or full path), the file's folder, then a name search (case-insensitive) under it and up to two parent folders; missing samples reported | | | F-EXS-09 |
| Consolidated single-file samples | §5 | must (WAV/AIFF) / skip (CAF/AAC) | zones sharing one file with their own ranges | | compressed (CAF/AAC) samples reported, zones skipped | F-EXS-05 |
| Parameters: master volume/pan, tune, transpose, bend, polyphony, legato | §6 | must | transpose, coarse, fine, bend up/down (−1 = same as up), polyphony, legato (groups legato) | master volume read as dB, master pan on the zone pan scale (units unconfirmed; reported) | | F-EXS-08 |
| Parameters: amp and filter envelopes | §6 | must | | amp envelope by type with times `10 × (p/127)⁴` s and the high-velocity attack; filter envelope stored with depth 0 (modulation-matrix enumerations undocumented; reported) | | F-EXS-08 (sound checked) |
| Parameters: filter 1 | §6 | approx | | type, cutoff 20 Hz–20 kHz exponential, resonance 0–24 dB, key tracking (scales unconfirmed; SPEC-GAP; reported) | | F-EXS-08 (the model only: the fixture has no unfiltered note to compare) |
| Parameters: filter 2, LFOs, glide, velocity curve, random pitch, crossfade | §6 | approx | | | kept as data in `extra["exs.params"]` (reported when set) | F-EXS-08 |
| Modulation matrix | §6 | data | kept in `extra["exs.params"]` | | not mapped (enumerations undocumented) | F-EXS-08 |
| Malformed input never panics | §1 | must | 2026-11-05 | | | `malformed_files_are_refused_never_panic`, `fuzz_exs`, `fuzz_exs_mutated` |

## Formats from other importers

Formats added through the importer hook (M3/T6, [`docs/formats/importer-hook.md`](../formats/importer-hook.md)) are built and documented outside this repository; their parity is kept with them. The hook itself is proven by a test importer in the test suite (`importer_hook_end_to_end_over_mcp`).

