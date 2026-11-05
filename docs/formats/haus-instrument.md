# Haus Sampler instrument format

Status: **contract, M3/T0 (2026-11-05).** File version 1. Model: `ph_core::HausInstrument`
(`crates/ph-core/src/sampler.rs`); file code: `ph_formats::instrument`
(`crates/ph-formats/src/instrument.rs`). Our own format: nothing here comes from another
sampler's documentation or code.

A Haus Sampler instrument is a multi-sample instrument: **zones** (one sample each, mapped to a
key and velocity range) collected in **groups** (shared envelopes, filter, LFOs, voices, round
robin, output bus, effects), plus **articulations** (keyswitches), a **modulation matrix** and
global **settings**. The same model lives in two places:

- in a **project** (`project.json`, format 3), where it is edited, undone and played;
- in an **instrument folder** on disk (`instrument.toml`), to keep and share instruments.

## 1. The instrument folder

```
<Name>/
  instrument.toml   the zone map and settings (this document)
  samples/          the audio files the zones play
  ui/               reserved for M3b (instrument skins); not read or written in M3
  scripts/          reserved for M3b (Luau, KSP); not read or written in M3
```

- Sample paths in `instrument.toml` are **relative to the folder**, with forward slashes, no
  `..`, no drive or root (`samples/piano_C4_f.wav`). Anything else is refused on load, so an
  instrument folder can be moved or zipped as a whole.
- Sample files are any audio `ph-formats` decodes: WAV, AIFF, FLAC, Ogg Vorbis, MP3.
- Saving writes `instrument.toml` through a temporary file and a rename; saving the same
  instrument twice gives the same bytes. Copying sample files is the saver's job
  (`sampler.save_native`, M3/T3).
- The text encoding is UTF-8 TOML 1.0.

## 2. How a project refers to an instrument

**Decision (M3/T0): the zone map always lives in the project. Samples are either linked from
a library folder or collected into the project.**

- `Project.instruments` holds every `HausInstrument` (with its groups and zones) under a stable
  `InstrumentId`. A channel plays one with `InstrumentSpec::MultiSampler { instrument: Some(id),
  library, params }`. `library` is informational ("loaded from"); `params` are the processor
  parameters (`volume`, `pan`, `transpose`).
- The instrument's `folder` says where `File` sample paths resolve:
  - **linked**: an absolute path inside an allowed library folder (`D:/Libraries/Grand`); the
    samples stay where they are;
  - **collected**: a project-relative path, `instruments/<name>`; the samples were copied into
    the project folder;
  - `None`: the project folder itself.
  A zone can also play a project **registry** sample (`sample: 3`), like audio clips do.

Why the map is embedded, not referenced:
- every zone edit is an ordinary undoable patch with per-author undo, and agents edit zones
  through `ph-commands` like any other object (a reference to an external file could not be
  undone or co-edited);
- reopening a project gives exactly the instrument it was saved with, even if the library's
  own `instrument.toml` (or `.sfz`) changed since;
- the engine compiles the instrument from the project snapshot alone, so live and offline
  renders see the same thing.

What this means for each operation:

| operation | effect |
|---|---|
| **save** (`file.save`) | `project.json` holds the full zone map (format 3). Library folders are never written. A linked instrument's samples are not copied. |
| **reopen** (`file.open`) | The zone map comes from `project.json`; samples resolve against `folder`. A missing library folder makes those zones silent with a warning (`get_state` warnings); nothing is lost. |
| **collect into project** (`sampler.collect`, M3/T3) | Copies every file the instrument plays into `<project>/instruments/<name>/samples/` (`(2)`, `(3)` on clashes), rewrites the zone paths if needed and sets `folder` to `instruments/<name>`, as one undoable edit. After that the project no longer needs the library. Registry samples already live in the project and are left alone. |
| **save as native** (`sampler.save_native`, M3/T3) | Writes an instrument folder: `instrument.toml` plus copies of every sample in `samples/` (registry samples included, converted to `File` paths in the written file only). The project is not changed. |
| **load native** (`sampler.load`, M3/T3) | Reads an instrument folder into `Project.instruments` (fresh ids), linked (`folder` = the folder) or, with `collect: true`, collected. |
| **import** (`sampler.import`, M3/T3–T6) | An importer turns an SFZ/SF2/... file into a `HausInstrument` plus a sample list; linked when every sample is a plain file in the library, otherwise (samples inside a container: SF2, monoliths, NCW; generated SFZ `*sine`/`*silence`) written into the project as collected, with `import-report.md` beside it. `collect: true` always collects. A **folder of samples** is mapped by the note names in the file names (rule: `ph_sampler_formats::automap`). **SF2/SF3 (M3/T4):** a SoundFont has no sample files, so its samples (cut out of `smpl`, 24-bit with `sm24`, SF3 Ogg Vorbis decoded, stereo pairs joined) are written as 32-bit float WAV files into `instruments/<name>/samples/` (`SampleSource::Audio`); SF2 imports are always collected, and `preset` picks the preset (`bank:program` or a name). A `.dsbundle` folder is an instrument (its first preset), not a folder of samples. |

Removing a channel leaves its instrument in `Project.instruments` (harmless, undo-safe);
`sampler.remove` for unused instruments is left to M3/T7.

## 3. Ids and object paths (project)

| object | path | value | ids |
|---|---|---|---|
| instrument | `instrument/<i>` | the instrument without `groups` and `zones` | `InstrumentId`, project-wide, never reused |
| group | `instrument/<i>/group/<g>` | `Group` | `GroupId`, unique across the project, never reused |
| zone | `instrument/<i>/zone/<z>` | `Zone` | `ZoneId`, unique across the project, never reused |
| articulation | inside the instrument value | `Articulation` | `ArticulationId`, per instrument: the lowest unused number |

Model invariants (enforced on every patch and on load): a zone's group is in the same
instrument; a registry sample a zone plays exists; a group's articulations and a route's group
exist; an instrument is deleted only without groups, zones and channels playing it; a group
only without zones and scoped routes; a registry sample only while no zone plays it.

In an instrument file the ids are the file's own: groups and zones carry `id`, zones and routes
refer to group ids. Loading into a project gives everything fresh project ids
(`Edit::add_instrument` remaps zones and scoped routes).

## 4. `instrument.toml`

Top-level keys, in file order. Types are TOML types; "absent" is the default when a key is
left out.

| key | type | default | meaning | since |
|---|---|---|---|---|
| `format` | string | required | always `"patternhaus-instrument"` | 1 |
| `version` | integer | required | file version; 1 now. Older versions are migrated on load; newer ones are refused ("update Patternhaus"). | 1 |
| `name` | string | required | instrument name | 1 |
| `[settings]` | table | all defaults | §4.1 | 1 |
| `[source]` | table | absent | §4.2 | 1 |
| `[extra]` | table of strings | empty | opaque data kept for M3b, keyed `<format>.<what>` (`ksp.script`, `dspreset.ui`); never interpreted in M3 | 1 |
| `[[import_note]]` | array of tables | empty | §4.3 | 1 |
| `[[articulation]]` | array of tables | empty | §4.4 | 1 |
| `[[modulation]]` | array of tables | empty | §4.5, applied in order | 1 |
| `[[group]]` | array of tables | empty | §4.6 | 1 |
| `[[zone]]` | array of tables | empty | §4.7 | 1 |

Unknown top-level keys are refused. Unknown keys inside tables are ignored by version 1
readers; writers never produce them.

### 4.1 `[settings]`

| key | type | unit / range | default |
|---|---|---|---|
| `volume_db` | float | dB, -144..=24 | 0.0 |
| `pan` | float | -1 (left) ..= 1 (right) | 0.0 |
| `transpose` | integer | semitones, -48..=48 | 0 |
| `tune_cents` | float | cents, -100..=100 | 0.0 |
| `voice_limit` | integer | voices, 1..=1024 | 256 |
| `bend_up`, `bend_down` | float | semitones, 0..=48 | 2.0 |
| `default_articulation` | integer | an articulation `id` | absent = the first articulation |
| `preload_frames` | integer | frames of each sample kept in memory; the rest streams | absent = engine default (32768) |

### 4.2 `[source]` (provenance)

| key | type | meaning |
|---|---|---|
| `format` | string | `native`, `sfz`, `sf2`, `sf3`, `dspreset`, `exs`, `nki`, `nkm`, ... |
| `file` | string | the file it was imported from, as given to the importer |
| `importer_version` | integer | version of the importer that read it |

### 4.3 `[[import_note]]`

| key | type | default | meaning |
|---|---|---|---|
| `kind` | string | required | `approximated` (mapped to the nearest thing), `skipped` (dropped), `kept` (kept as data, not used yet: scripts, UI) |
| `what` | string | required | the opcode, chunk or feature (`opcode:fil_type=lpf_6p`) |
| `detail` | string | `""` | plain words for a musician |
| `count` | integer | 1 | occurrences |

### 4.4 `[[articulation]]`

| key | type | default | meaning |
|---|---|---|---|
| `id` | integer | required | unique in the instrument |
| `name` | string | required | `sustain`, `staccato`, ... |
| `trigger` | table | required | `{ kind = "key", key = 24 }`: a keyswitch (latching; the key makes no sound), or `{ kind = "cc", controller = 32, lo = 0.5, hi = 1.0 }`: selected while the controller is in `lo..=hi` (0..=1) |
| `color` | integer | absent | keyboard colour, 0xRRGGBB |

Groups join articulations through their `articulations` list; a group with an empty list plays
in all of them. An instrument without articulations plays every group.

### 4.5 `[[modulation]]` (the modulation matrix)

`target += depth × curve(source)`, per voice.

| key | type | default | meaning |
|---|---|---|---|
| `source` | table | required | `{ kind = ... }`, one of: `velocity` (0..1), `key` ((key − 60) / 64, bipolar), `cc` + `controller` (0..1), `pitch_bend` (−1..1), `aftertouch` (0..1), `poly_pressure` (0..1), `lfo` + `index` (the group's LFO 0..=3, −1..1), `envelope` + `index` (0 = amp, 1 = filter; 0..1), `random` (one seeded draw per note, 0..1) |
| `target` | string | required | `pitch` (depth in cents), `volume` (dB), `pan` (pan units), `cutoff` (cents), `resonance` (dB), `sample_start` (frames of the file) |
| `depth` | float | required | in the target's unit at full source |
| `curve` | string | `linear` | `linear`, `convex` (x²), `concave` (√x), `switch` (0 below one half, 1 from one half) |
| `group` | integer | absent | only voices of this group id; absent = every group |

### 4.6 `[[group]]`

| key | type | unit / range | default |
|---|---|---|---|
| `id` | integer | unique in the file | required |
| `name` | string | | required |
| `color` | integer | 0xRRGGBB | 0 |
| `amp_env` | table | DAHDSR, §4.6.1 | defaults |
| `filter_env` | table | DAHDSR; drives cutoff by `filter.env_depth` | absent |
| `filter` | table | §4.6.2 | absent = no filter |
| `lfos` | array of tables | at most 4; `{ shape, rate, delay, fade }`: shape `sine` (default), `triangle`, `square`, `saw_up`, `saw_down`, `random` (seeded sample and hold); `rate` Hz 0.01..=100; `delay`, `fade` seconds | empty |
| `voice_limit` | integer | ≥ 1 | absent = only the instrument's limit |
| `stealing` | string | `oldest`, `quietest`, `newest`, `none` (the new note does not sound) | `oldest` |
| `polyphony` | string | `poly`, `mono` (a new note retriggers), `legato` (an overlapping note glides without retriggering) | `poly` |
| `portamento` | float | glide seconds for mono/legato, 0..=10 | 0.0 |
| `round_robin` | string | `sequential` (positions 0, 1, 2, ... in turn) or `random` (seeded; never the same position twice in a row when there are two or more) | `sequential` |
| `output` | integer | output bus: 0 = main, 1..=15 = extra outputs (multi-out) | 0 |
| `effects` | array of tables | built-in insert chain: `{ name, params = { id = value }, bypass = false, mix = 1.0 }`; names and parameter ids as the mixer's built-in effects | empty |
| `articulations` | array of integers | articulation ids this group plays in | empty = all |
| `volume_db` | float | dB, -144..=24 | 0.0 |
| `extra` | table of strings | opaque M3b data, as the instrument's | empty |

#### 4.6.1 Envelopes

| key | type | unit | default |
|---|---|---|---|
| `delay`, `attack`, `hold`, `decay` | float | seconds, 0..=60 | 0.0 |
| `sustain` | float | level, 0..=1 (linear) | 1.0 |
| `release` | float | seconds, 0..=60 | 0.001 |

#### 4.6.2 `filter`

| key | type | unit / range | default |
|---|---|---|---|
| `kind` | string | `low_pass12`, `low_pass24`, `high_pass12`, `high_pass24`, `band_pass`, `notch` | required |
| `cutoff` | float | Hz, 10..=22000 | required |
| `resonance` | float | dB, 0..=40 | 0.0 |
| `key_track` | float | cents per key above C4 (100 = follows the keyboard), 0..=1200 | 0.0 |
| `env_depth` | float | cents of cutoff at full filter envelope, -9600..=9600 | 0.0 |

### 4.7 `[[zone]]`

| key | type | unit / range | default |
|---|---|---|---|
| `id` | integer | unique in the file | required |
| `group` | integer | a group `id` | required |
| `sample` | string | path relative to the folder (in a project also an integer: a registry sample id) | required |
| `keys` | array `[lo, hi]` | MIDI keys, inclusive, 0..=127 | `[0, 127]` |
| `velocity` | array `[lo, hi]` | MIDI velocities, inclusive, 0..=127; a note's 0..1 velocity maps to `round(v × 127)` | `[0, 127]` |
| `root_key` | integer | the key that plays the sample at its recorded pitch | required |
| `tune` | integer | semitones, -48..=48 | 0 |
| `fine` | float | cents, -100..=100 | 0.0 |
| `volume_db` | float | dB, -144..=24 | 0.0 |
| `pan` | float | -1..=1 | 0.0 |
| `start` | integer | first frame played (frames of the file, at its own rate) | 0 |
| `end` | integer | one past the last frame played | absent = the file's end |
| `loop` | table | §4.7.1 | absent = no loop |
| `reverse` | bool | plays `start..end` backwards | false |
| `trigger` | string | `attack` (note-on) or `release` (note-off, with the note's velocity) | `attack` |
| `one_shot` | bool | ignores note-off and plays to the end (drums) | false |
| `release_decay_db` | float | release trigger only: dB quieter per second the note was held, 0..=200 | 0.0 |
| `round_robin` | integer | position in its group's cycle, 0-based; the cycle length is the highest position + 1 | absent = plays every time |
| `cc_conditions` | array of tables | `{ controller, lo, hi }` (0..1): plays only while every controller is in range | empty |
| `crossfades` | array of tables | §4.7.2 | empty |

#### 4.7.1 `loop`

| key | type | unit | default |
|---|---|---|---|
| `mode` | string | `forward` or `ping_pong` | `forward` |
| `start` | integer | first looped frame; inside `start..end` of the zone | required |
| `end` | integer | one past the last looped frame | required |
| `crossfade` | integer | frames before `end`, at most the loop length | 0 |
| `on_release` | string | `continue` (keep looping through the release) or `exit` (a sustain loop: leave it at note-off and play on to the end) | `continue` |

#### 4.7.2 `crossfades`

| key | type | meaning |
|---|---|---|
| `source` | table | `{ kind = "velocity" }` or `{ kind = "cc", controller = 1 }` |
| `in_lo`, `in_hi` | float | gain rises from 0 at `in_lo` to 1 at `in_hi` (0..1) |
| `out_lo`, `out_hi` | float | gain falls from 1 at `out_lo` to 0 at `out_hi`; `in_lo <= in_hi <= out_lo <= out_hi`, equal ends mean no fade on that side |
| `curve` | string | `power` (equal power, default) or `gain` (linear) |

## 5. Example

```toml
format = "patternhaus-instrument"
version = 1
name = "Felt Piano"

[settings]
voice_limit = 128
default_articulation = 0

[[articulation]]
id = 0
name = "sustain"

[articulation.trigger]
kind = "key"
key = 24

[[group]]
id = 0
name = "Sustain"
articulations = [0]

[group.amp_env]
attack = 0.002
release = 0.6

[[zone]]
id = 0
group = 0
sample = "samples/C4_p.wav"
keys = [55, 66]
velocity = [0, 80]
root_key = 60

[[zone]]
id = 1
group = 0
sample = "samples/C4_f.wav"
keys = [55, 66]
velocity = [81, 127]
root_key = 60

[zone.loop]
start = 48000
end = 96000
crossfade = 2048
```

## 6. Versions and migrations

- **File version 1** (M3/T0): everything above.
- `ph_formats::instrument` keeps a `MIGRATIONS` list like the project format: a step upgrades
  version `n` to `n + 1` in place on the parsed TOML table; adding a field with a default needs
  no step. Every step comes with a fixture of the old version in the tests.
- Projects carry the model under their own `format_version`: **3** added `instruments`, the
  `instrument`/`group`/`zone` id counters and `MultiSampler.instrument` (migration 2 → 3 only
  checks; M1 and M2 projects load unchanged).
