# SoundFont 2 / SoundFont 3 (`.sf2`, `.sf3`): format specification

**Status:** M3/T1, first edition (2026-11-05). Written for the SF2/SF3 importer (M3/T4),
which is implemented from this document alone.

**Clean-room statement.** SF2 has a public technical specification (E-mu/Creative, version
2.04, 2002). The facts below are restated from it in our own words, with section numbers so
the implementer can check them. SF3 has no formal specification; its facts come from the
FluidSynth project's public wiki page. No synthesiser or converter source code was read.
The specification PDF itself is not committed.

**Evidence tags:** **[S]** per spec (SF2 2.04, section given as §n); **[D]** documented, not
seen (a secondary public source, named); **[I]** inferred; **[C]** confirmed on files
(none yet: no corpus examined; generated fixtures in section 12 will confirm the
reader); **[PH]** a Patternhaus decision where the format leaves a choice.

**Cited sources:**

- [SF2] SoundFont Technical Specification, version 2.04 (2002-09-10), E-mu Systems.
  Public copy: http://www.synthfont.com/sfspec24.pdf. Sections referenced as §n.
- [SF3] FluidSynth wiki, "SoundFont3Format": https://www.fluidsynth.org/wiki/SoundFont3Format/
  (also at https://github.com/FluidSynth/fluidsynth/wiki/SoundFont3Format).
- [SF3-MAN] Debian manual page of `sf3convert` (states that SF3 has no formal spec and that
  the MuseScore tool is the reference writer): https://dyn.manpages.debian.org/trixie/sf3convert/sf3convert.1
- [MS-SF3] MuseScore forum thread "Documenting and possibly extending the SF3 format":
  https://musescore.org/en/node/299957

Conventions: all integers little-endian. `u8/u16/u32`, `i8/i16` as in `flp.md`. `ZSTR` is
a NUL-terminated ASCII string. "Frame" = one sample point of a mono sample.

---

## 1. RIFF container [S §4]

```
RIFF <size> 'sfbk'
  LIST <size> 'INFO'   { sub-chunks, section 2 }
  LIST <size> 'sdta'   { [smpl] [sm24] }
  LIST <size> 'pdta'   { phdr pbag pmod pgen inst ibag imod igen shdr }
```

- Each chunk: 4-byte ASCII id, `u32` size (excluding the 8-byte header), data, then one pad
  byte if size is odd (RIFF rule) [S].
- The three LISTs appear in the order INFO, sdta, pdta [S]. A reader should still locate
  them by id [PH].
- The nine pdta sub-chunks are all mandatory and appear in the order shown [S §7.1].
  A missing pdta sub-chunk, or one whose size is not a multiple of its record size, means
  the file is structurally unsound and is rejected [S §7.2–7.10].
- Unknown chunks anywhere are skipped and reported [PH].

---

## 2. INFO list [S §5]

| Id | Content | Required |
|---|---|---|
| `ifil` | version: `u16 major`, `u16 minor` (exactly 4 bytes; else reject) | yes |
| `isng` | ZSTR target sound engine, e.g. `EMU8000` | yes (default `EMU8000` if missing) |
| `INAM` | ZSTR bank name | yes |
| `irom` | ZSTR sound-ROM name | no |
| `iver` | ROM version, `u16 major`, `u16 minor` | no |
| `ICRD` | ZSTR creation date | no |
| `IENG` | ZSTR engineers | no |
| `IPRD` | ZSTR target product | no |
| `ICOP` | ZSTR copyright | no |
| `ICMT` | ZSTR comments | no |
| `ISFT` | ZSTR tools used | no |

- Strings are at most 256 bytes including the NUL, padded to even length [S §5]. Read up to
  the first NUL; treat as Latin-1 [PH].
- **Version:** `ifil.major` = 2 for SF2 (minor 0, 1 or 4 seen in the wild [I]); **3 for SF3**
  [SF3]. A major above 3 is reported but the reader tries anyway [PH].
- Keep INAM, ICOP, IENG, ICMT, ICRD as instrument metadata in the native format [PH].

---

## 3. Sample data (`sdta`) [S §6]

- `smpl`: 16-bit signed PCM, mono, all samples concatenated [S]. Each sample is followed by
  at least **46 zero frames** [S §7.10].
- `sm24` (2.04 and later): one byte per `smpl` frame holding the **low 8 bits** of a 24-bit
  value; the 24-bit frame is `(smpl << 8) | sm24` [S §6.2].
  - Ignore `sm24` if `ifil` is below 2.04, if `smpl` is missing, or if its size is not
    exactly half of `smpl`'s byte size, plus 1 when that half is odd (the pad byte)
    [S §6.2].
- Samples may also live in a sound **ROM** (section 6.3).

---

## 4. Hydra records (`pdta`) [S §7]

Every list ends with a **terminal record** that is never used as data, except to give the
end index of the previous entry [S].

### 4.1 `phdr`: presets, 38 bytes [S §7.2]

| Off | Type | Field |
|---|---|---|
| 0 | char[20] | name, NUL-padded |
| 20 | u16 | preset (MIDI program) 0–127 |
| 22 | u16 | bank 0–127, **128 = percussion bank** |
| 24 | u16 | first index into `pbag` |
| 26 | u32 | library (reserved) |
| 30 | u32 | genre (reserved) |
| 34 | u32 | morphology (reserved) |

- At least two records (one preset + terminal `EOP`). Bag indices must be monotonic and the
  terminal's index × 4 + 4 must equal the `pbag` size; otherwise reject [S].
- Duplicate (bank, preset): the first one is active; keep the others [S].
- Preset or bank out of range: keep but do not map to a program [S].
- A preset with no zones is ignored [S].

### 4.2 `pbag` / `ibag`: zones, 4 bytes [S §7.3, §7.7]

`u16 generator index`, `u16 modulator index`. Zone *i* of a preset owns generators
`[gen(i), gen(i+1))` and modulators `[mod(i), mod(i+1))`. Indices monotonic; terminal
record sizes must match `pgen`/`pmod` (`igen`/`imod`) [S].

### 4.3 `pmod` / `imod`: modulators, 10 bytes [S §7.4, §7.8]

| Off | Type | Field |
|---|---|---|
| 0 | u16 | source (section 8.1) |
| 2 | u16 | destination: a generator id, or, if bit 15 is set, a **link** to the modulator at index (bits 0–14) relative to the zone's first modulator |
| 4 | i16 | amount |
| 6 | u16 | amount source (section 8.1) |
| 8 | u16 | transform: 0 linear, 2 absolute value |

- Two modulators in one zone with the same source, destination and amount source: the
  **later** one wins, the earlier is ignored [S §7.4].
- Unknown sources, destinations or transforms: ignore that modulator [S].

### 4.4 `pgen` / `igen`: generators, 4 bytes [S §7.5, §7.9]

`u16 generator id`, then a 2-byte amount interpreted as one of:
- **range**: `u8 lo`, `u8 hi` (for `keyRange`, `velRange`);
- **i16** signed (most generators);
- **u16** (`instrument`, `sampleID`, `sampleModes`).

Ordering rules [S §7.5, §7.9]:
- `keyRange` (43), if present, must be the **first** generator of a zone; elsewhere ignore it.
- `velRange` (44) may be preceded only by `keyRange`; elsewhere ignore it.
- In a preset zone the last generator must be `instrument` (41); in an instrument zone,
  `sampleID` (53). Generators after it are ignored.
- A zone other than the first that lacks its terminal generator is ignored.
- The **first** zone is a **global zone** if it lacks the terminal generator; it may also
  have only modulators. A global zone with neither is ignored.
- Repeated generator ids in one zone: the last occurrence wins [I].
- Generators that are instrument-only (section 5, marked "I") found in a preset zone are
  ignored [S §8.5].
- `instrument`/`sampleID` ≥ the terminal index: reject the file [S].

### 4.5 `inst`: instruments, 22 bytes [S §7.6]

`char[20] name`, `u16 first ibag index`. Terminal `EOI`. Same monotonic rules.

### 4.6 `shdr`: samples, 46 bytes [S §7.10]

| Off | Type | Field |
|---|---|---|
| 0 | char[20] | sample name |
| 20 | u32 | start: frame index of the first frame in `smpl` |
| 24 | u32 | end: index of the first of the 46 zero frames after the sample (so **exclusive**) |
| 28 | u32 | loop start: index of the first frame in the loop |
| 32 | u32 | loop end: index of the frame **after** the loop (exclusive; the frame "equivalent to" loop start) |
| 36 | u32 | sample rate in Hz |
| 40 | u8 | original pitch: MIDI key of the recording; 255 = unpitched; 128–254 illegal; for 255 or illegal, use 60 |
| 41 | i8 | pitch correction in cents (applied on playback) |
| 42 | u16 | sample link: index of the stereo partner (section 6.2) |
| 44 | u16 | sample type (section 6) |

- All indices are **absolute** within `smpl`, not relative to the sample's start [S].
- Recommended constraints (min length 48 frames, min loop 32, 8 frames before loop start
  and after loop end) may be violated by real files; play anyway [S says hardware may
  refuse; PH: never refuse].
- Sample rate 0 is illegal: use 44100 and report [S says "nearest practical"; PH picks].
- Duplicate sample names are allowed; keep them [S].
- Terminal record `EOS`.

---

## 5. Generators [S §8.1.2, §8.1.3]

Kinds: **index** (instrument, sampleID), **range** (keyRange, velRange), **substitution**
(keynum, velocity), **sample** (address offsets, sampleModes, exclusiveClass,
overridingRootKey), **value** (all others) [S §8.1.1, §9.4].

Lvl: **P+I** valid at preset and instrument level; **I** instrument only (ignored in
presets) [S §8.5].

| Id | Name | Lvl | Unit | Range | Default | Meaning | M3 |
|---|---|---|---|---|---|---|---|
| 0 | startAddrsOffset | I | frames | ≥ 0 | 0 | added to shdr start | must |
| 1 | endAddrsOffset | I | frames | ≤ 0 | 0 | added to shdr end | must |
| 2 | startloopAddrsOffset | I | frames | any | 0 | added to loop start | must |
| 3 | endloopAddrsOffset | I | frames | any | 0 | added to loop end | must |
| 4 | startAddrsCoarseOffset | I | 32768 frames | ≥ 0 | 0 | × 32768 added to start | must |
| 5 | modLfoToPitch | P+I | cents | ±12000 | 0 | | must |
| 6 | vibLfoToPitch | P+I | cents | ±12000 | 0 | | must |
| 7 | modEnvToPitch | P+I | cents | ±12000 | 0 | | must |
| 8 | initialFilterFc | P+I | abs cents | 1500–13500 | 13500 | lowpass cutoff, Hz = 8.176 × 2^(c/1200); 13500 ≈ 19.9 kHz = open | must |
| 9 | initialFilterQ | P+I | cB | 0–960 | 0 | resonance peak above DC | must |
| 10 | modLfoToFilterFc | P+I | cents | ±12000 | 0 | | must |
| 11 | modEnvToFilterFc | P+I | cents | ±12000 | 0 | | must |
| 12 | endAddrsCoarseOffset | I | 32768 frames | ≤ 0 | 0 | | must |
| 13 | modLfoToVolume | P+I | cB | ±960 | 0 | tremolo depth, positive = attenuation at LFO +1 [I] | must |
| 14 | unused1 | — | | | | ignore | — |
| 15 | chorusEffectsSend | P+I | 0.1 % | 0–1000 | 0 | | approximate (keep as data) |
| 16 | reverbEffectsSend | P+I | 0.1 % | 0–1000 | 0 | | approximate (keep as data) |
| 17 | pan | P+I | 0.1 % | −500 (left)–500 (right) | 0 | | must |
| 18–20 | unused2–4 | — | | | | ignore | — |
| 21 | delayModLFO | P+I | timecents | −12000–5000 | −12000 | | must |
| 22 | freqModLFO | P+I | abs cents | −16000–4500 | 0 (= 8.176 Hz) | | must |
| 23 | delayVibLFO | P+I | timecents | −12000–5000 | −12000 | | must |
| 24 | freqVibLFO | P+I | abs cents | −16000–4500 | 0 | | must |
| 25 | delayModEnv | P+I | timecents | −12000–5000 | −12000 | | must |
| 26 | attackModEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 27 | holdModEnv | P+I | timecents | −12000–5000 | −12000 | | must |
| 28 | decayModEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 29 | sustainModEnv | P+I | −0.1 % | 0–1000 | 0 | decrease below peak: 0 = full, 1000 = zero | must |
| 30 | releaseModEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 31 | keynumToModEnvHold | P+I | tc/key | ±1200 | 0 | hold time change per key from 60 | must |
| 32 | keynumToModEnvDecay | P+I | tc/key | ±1200 | 0 | | must |
| 33 | delayVolEnv | P+I | timecents | −12000–5000 | −12000 | | must |
| 34 | attackVolEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 35 | holdVolEnv | P+I | timecents | −12000–5000 | −12000 | | must |
| 36 | decayVolEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 37 | sustainVolEnv | P+I | cB attenuation | 0–1440 | 0 | sustain level as attenuation below peak | must |
| 38 | releaseVolEnv | P+I | timecents | −12000–8000 | −12000 | | must |
| 39 | keynumToVolEnvHold | P+I | tc/key | ±1200 | 0 | | must |
| 40 | keynumToVolEnvDecay | P+I | tc/key | ±1200 | 0 | | must |
| 41 | instrument | P only | index | | — | terminal generator of a preset zone | must |
| 42 | reserved1 | — | | | | ignore | — |
| 43 | keyRange | P+I | lo, hi | 0–127 | 0–127 | | must |
| 44 | velRange | P+I | lo, hi | 0–127 | 0–127 | | must |
| 45 | startloopAddrsCoarseOffset | I | 32768 frames | any | 0 | | must |
| 46 | keynum | I | key | 0–127 | −1 (unset) | the key used **for all key-dependent parameters** instead of the played key | must |
| 47 | velocity | I | vel | 1–127 | −1 (unset) | the velocity used instead of the played one | must |
| 48 | initialAttenuation | P+I | cB | 0–1440 | 0 | | must |
| 49 | reserved2 | — | | | | ignore | — |
| 50 | endloopAddrsCoarseOffset | I | 32768 frames | any | 0 | | must |
| 51 | coarseTune | P+I | semitones | ±120 | 0 | | must |
| 52 | fineTune | P+I | cents | ±99 | 0 | | must |
| 53 | sampleID | I only | index | | — | terminal generator of an instrument zone | must |
| 54 | sampleModes | I | flags | 0–3 | 0 | bits 0–1: 0 no loop, 1 loop continuously, 2 (unused) = no loop, 3 loop while key held then play to end | must |
| 55 | reserved3 | — | | | | ignore | — |
| 56 | scaleTuning | P+I | cents/key | 0–1200 | 100 | pitch change per key | must |
| 57 | exclusiveClass | I | id | 0–127 | 0 | non-zero: a new note in this class ends sounding notes of the same class **in the same preset** [S §8.1.2] | must |
| 58 | overridingRootKey | I | key | 0–127 | −1 (unset → shdr original pitch) | | must |
| 59 | unused5 | — | | | | ignore | — |
| 60 | endOper | — | | | | ignore | — |

Ids ≥ 61 are unknown: ignore and report [S: unknown enumerators are ignored, §10].

Units [S §9.3]:
- **timecents:** seconds = 2^(tc/1200); −12000 ≈ 1 ms. Treat **−32768** as exactly 0 s [I,
  common writer convention].
- **absolute cents** (frequencies): Hz = 8.176 × 2^(c/1200).
- **cB** (centibels): 10 cB = 1 dB; attenuation in dB = cB/10.

### 5.1 Effective sample addresses [S §8.1.2]

```
start      = shdr.start     + gen0 + 32768 × gen4
end        = shdr.end       + gen1 + 32768 × gen12
loop_start = shdr.loopstart + gen2 + 32768 × gen45
loop_end   = shdr.loopend   + gen3 + 32768 × gen50
```

Clamp so that `shdr.start ≤ start < end ≤ shdr.end` and loop points lie inside
`[start, end]`; if the loop becomes empty, play unlooped [PH]. Offsets of the coarse
generators are only legal at instrument level [S §8.5].

### 5.2 Pitch [S §8.1.2, §9.1.1]

```
root   = overridingRootKey if set, else shdr.original_pitch (255 or 128–254 → 60)
key    = keynum generator if set, else the played key
cents  = (key − root) × scaleTuning + coarseTune × 100 + fineTune + shdr.pitch_correction
         + modulators (pitch wheel etc.) + LFO and envelope terms
ratio  = 2^(cents/1200) × shdr.sample_rate / engine_rate
```

`scaleTuning` measures from the root key [I: the usual reading].

---

## 6. Sample types and links [S §7.10]

### 6.1 Sample type values

| Value | Name |
|---|---|
| 1 | mono |
| 2 | right |
| 4 | left |
| 8 | linked (not fully defined) |
| 0x8001 | ROM mono |
| 0x8002 | ROM right |
| 0x8004 | ROM left |
| 0x8008 | ROM linked |
| **0x0010 bit** | SF3: compressed (section 9) |

Bit 15 = ROM; the low four bits name the channel role; they are mutually exclusive [S].
SF3 adds bit 4 [SF3]. Any other bits: report, treat as mono [PH].

### 6.2 Stereo pairs [S]

- A **left** sample's link is the shdr index of its **right** partner, and vice versa.
- Both play fully in sync; **pitch comes from the right sample's generators** [S]; every other
  generator applies per sample as normal; left/right placement is done by the `pan`
  generator in each zone.
- Partners should be in the same instrument; one instrument should never be able to
  trigger two instances of the same pair [S].
- **[PH]:** each instrument zone becomes one mono voice. When an instrument has two zones
  with identical key/velocity ranges whose samples are linked partners, the importer joins
  them into **one stereo zone** in the native format (left = the left sample, pitch and
  loop from the right zone) and records the join in the report. Unjoined linked samples
  play as mono voices.
- `linked` (8) circular lists: treat as mono and report [PH].
- Some SF3 writers set the link to 0 [SF3]: do not join on links alone in SF3; also accept a
  pair by matching names ending in `L`/`R` or `(L)`/`(R)` only when the zones match [PH].

### 6.3 ROM samples [S]

ROM samples refer to a hardware sound ROM named by `irom`. Their data is not in the file.
A ROM sample without `irom` makes the file structurally defective [S]. **[PH]:** zones using
ROM samples are skipped and reported ("needs sound ROM"); the rest of the file loads.

---

## 7. Zone resolution: presets, instruments and summing [S §8.5, §9.4]

A note (key k, velocity v) on preset P:

1. For each **preset zone** of P (excluding its global zone) whose keyRange contains k and
   velRange contains v, take its instrument I.
2. For each **instrument zone** of I (excluding its global zone) whose keyRange and velRange
   contain k and v, start one voice with the zone's sample.
   Preset ranges act as an **intersection filter** with instrument ranges; they are never
   added [S §9.4].
3. The voice's generator values:
   - **Instrument level is absolute.** Start with the defaults (section 5); the
     instrument **global** zone replaces defaults; the instrument **local** zone replaces
     both [S].
   - **Preset level is relative** (value generators only). The preset **global** zone's
     value is replaced by the preset **local** zone's value for the same generator; the
     result is **added** to the instrument-level value [S].
   - Range, index, substitution and sample generators are never added; sample and
     substitution generators at preset level are ignored [S].
4. Clamp each summed value to the generator's range (section 5) [I].

Worked example from [S §8.5] restated: an instrument zone with attackVolEnv 1200 tc in a
preset zone with attackVolEnv 2400 tc gives 3600 tc (8 s).

**Import mapping [PH]:** SF2 has no "instrument file" boundary like SFZ. The importer:
- lists presets (bank:program, name) and imports **one preset** per Haus Sampler instrument
  (the caller chooses; default: the first by bank then program);
- flattens each (preset zone × instrument zone) pair into one native zone with fully
  summed generators and intersected ranges;
- keeps the preset/instrument/zone names as group names (one group per instrument zone
  set, named after the instrument).

---

## 8. Modulators [S §8.2–8.4, §9.5]

### 8.1 Source enumeration (u16) [S §8.2]

| Bits | Field |
|---|---|
| 0–6 | index |
| 7 | CC flag: 0 = general controller palette, 1 = MIDI CC number = index |
| 8 | direction: 0 min→max, 1 max→min |
| 9 | polarity: 0 unipolar (0…1), 1 bipolar (−1…1) |
| 10–15 | type: 0 linear, 1 concave, 2 convex, 3 switch. Any of bits 11–15 set → ignore the modulator |

General palette (CC flag 0) [S §8.2.1]: 0 no controller (value 1), 2 note-on velocity,
3 note-on key, 10 poly pressure, 13 channel pressure, 14 pitch wheel, 16 pitch wheel
sensitivity (RPN 0), 127 link (output of another modulator; not valid as an amount
source). Others reserved: ignore the modulator.

MIDI CC palette: indices 0, 6, 32, 38, 98–101, 120–127 are illegal; 33–63 are reserved LSBs
of 1–31. Ignore modulators that use them [S §8.2.1].

Input normalisation [S §9.5.3]: a 7-bit controller value x maps to x/128 (so 127 →
127/128); the pitch wheel (14-bit) maps to x/8192 − 1 for bipolar use.

Curves [S §8.2.4, §9.5.2]:
- linear: the value itself;
- concave: a logarithmic curve from 0 to 1, used for velocity and volume:
  `concave(x) = −(20/96) × log10((1 − x)²)` clamped to [0, 1] [I: the commonly used
  reading of the spec's formula, which is stated loosely];
- convex: `1 − concave(1 − x)` [S: mirror of concave];
- switch: 0 below 0.5, 1 at or above [S].
Direction reverses the input (1 − x); polarity maps unipolar u to bipolar `2u − 1` [S].

### 8.2 Evaluation [S §9.5.1]

Modulator output = `transform(source(x) × amount × amount_source(y))`. Each destination's
value = generator value + sum of the outputs of all modulators targeting it. Transform 2 is
absolute value. Linked modulators (destination bit 15) feed their output as the **source**
of the target modulator; circular or out-of-range links are ignored [S §7.4].

### 8.3 Hierarchy of modulators [S §9.5.1]

"Identical" means same source, destination, amount source **and transform**.
- Default modulators apply at instrument level.
- An instrument **global** zone modulator identical to a default replaces it; one that
  differs adds.
- An instrument **local** zone modulator identical to a default or global one replaces it;
  otherwise adds.
- Preset-level modulators **add** to instrument-level ones (an identical preset modulator
  adds its amount to the instrument modulator's amount); preset local replaces preset
  global when identical.

### 8.4 Default modulators [S §8.4]

| # | Source | Destination | Amount | Amount source | Meaning |
|---|---|---|---|---|---|
| 1 | 0x0502: velocity, concave, unipolar, max→min | 48 initialAttenuation | 960 cB | none | soft notes up to 96 dB quieter |
| 2 | 0x0102: velocity, linear, unipolar, max→min | 8 initialFilterFc | −2400 cents | none | soft notes darker |
| 3 | 0x000D: channel pressure, linear, unipolar | 6 vibLfoToPitch | 50 cents | none | |
| 4 | 0x0081: CC1, linear, unipolar | 6 vibLfoToPitch | 50 cents | none | mod wheel vibrato |
| 5 | 0x0582: CC7, concave, unipolar, max→min | 48 initialAttenuation | 960 cB | none | volume |
| 6 | 0x028A: CC10, linear, **bipolar** | **17 pan** | 1000 (0.1 %) | none | pan. The spec text names "Initial Attenuation" here, which contradicts its own description; pan is meant [S, internal contradiction; I for the resolution] |
| 7 | 0x058B: CC11, concave, unipolar, max→min | 48 initialAttenuation | 960 cB | none | expression |
| 8 | 0x00DB: CC91, linear, unipolar | 16 reverbEffectsSend | 200 | none | |
| 9 | 0x00DD: CC93, linear, unipolar | 15 chorusEffectsSend | 200 | none | |
| 10 | 0x020E: pitch wheel, linear, bipolar | pitch (initial pitch) | 12700 cents | 0x0010 pitch wheel sensitivity | bend × range |

- They are implicit; files override them by adding identical modulators (with amount 0 to
  disable) [S].
- The old AWE32 rule that #2 applies only when the attack is under 7 ms was removed in 2.01
  [S §8.4.2].
- Modulator #10's destination "initial pitch" is not a numbered generator: it is the voice's
  pitch summing node [S]. Pitch wheel sensitivity defaults to 2 semitones (RPN 0) [I, MIDI
  convention], so 12700 × 2/128 ≈ 198 cents ≈ ±2 semitones [I].

**Attenuation scaling [D]:** many players (e.g. FluidSynth and E-mu hardware) apply
`initialAttenuation` at **0.4 ×** its nominal cB value for compatibility with hardware. This
is not in the spec. **[PH]:** Patternhaus applies attenuation from generators at 0.4 ×
(matching the common hardware reading) and from modulators at full value, and records the
choice in the parity table as "approximated". Fixture F-SF2-04 pins our behaviour.

### 8.5 M3 support

M3 must support: all default modulators; velocity, key, CC (any), channel pressure and
pitch wheel sources; linear, concave, convex and switch curves; destinations pitch,
initialAttenuation, pan, initialFilterFc, initialFilterQ, vibLfoToPitch, modLfoToPitch,
modLfoToVolume, modLfoToFilterFc, and the envelope times. Linked modulators, poly-pressure
sources and the effects sends may be skipped (reported).

---

## 9. SF3 (compressed samples) [SF3]

- `ifil.major` = **3** [SF3].
- A sample whose type has **bit 4 (0x10)** set is compressed [SF3]. The flag says only "some
  encoding"; the reader must identify it from the data [SF3]. Ogg Vorbis is the format the
  reference writer produces [SF3-MAN, MS-SF3].
- For compressed samples the shdr fields change meaning [SF3]:
  - **start**: offset **in bytes** from the beginning of the `smpl` data to the first byte of
    the sample's compressed stream;
  - **end**: byte offset of the **last** byte of the stream (inclusive). Stream length =
    `end − start + 1` [SF3; I for the +1, which follows from "last byte"];
  - **loop start / loop end**: in **frames, relative to the start of the decoded sample**
    (not absolute in `smpl`) [SF3];
  - sample rate, original pitch and pitch correction keep their SF2 meaning [I];
  - address offset generators (0–4, 12, 45, 50) apply in decoded frames, relative to the
    decoded sample [I].
- Each sample is encoded separately; streams are written one after another [SF3].
- Uncompressed PCM samples may be mixed in the same file; **all PCM samples come first**
  in `smpl`, followed by the compressed streams [SF3].
- No 46-frame zero padding after compressed streams; `smpl` may have odd length, so the
  LIST may be unpadded [SF3]. The RIFF reader must therefore tolerate a missing pad byte
  at the end of `smpl`/`sdta` in SF3 files [PH].
- `sm24` still applies to PCM samples; compressed samples ignore it [SF3].
- Compressed samples **must be mono**; a non-mono or undecodable stream is invalid, and
  instruments and presets that use it should be rejected [SF3]. **[PH]:** skip the zones
  that use it and report, rather than rejecting the whole preset.
- Sample links: some writers leave them 0, others write them correctly; keep them [SF3].
- Encoding detection [PH]: stream begins with `OggS` and the first packet starts with
  `\x01vorbis` → Ogg Vorbis (supported, decoded by `ph-formats`); `OggS` with `OpusHead` →
  Opus (skip, report); `fLaC` → FLAC (decode if `ph-formats` reads FLAC from memory);
  anything else → unknown (skip, report).
- Files produced by an older, incompatible converter ("sf2convert" by cognitone) do not
  follow these rules and are not supported by FluidSynth [SF3]. **[PH]:** if offsets do not
  point at a recognisable stream, report "unsupported SF3 variant" and skip those samples.
- Decoding happens at import or on first use, not on the audio thread. For streaming, the
  importer writes decoded samples to the instrument's `samples/` folder as WAV/FLAC [PH].

---

## 10. Synthesis model (for the engine mapping) [S §9.1]

- **Loop modes** (`sampleModes`): 1 loops continuously, including during release; 3 loops
  while the key is held and, after release, plays through the loop end to the sample end;
  0 and 2 play once [S §8.1.2, §9.1.2].
- **Filter:** one resonant **low-pass**; the spec idealises it (the figure shows
  12 dB/oct); resonance in cB is the peak above DC gain; DC gain drops by half the
  resonance [S §9.1.3]. **[PH]:** map to Haus Sampler's 2-pole low-pass (12 dB/oct) and
  disable the filter when cutoff ≥ 13500 cents and Q = 0.
- **LFOs:** triangle, start at 0 after the delay, rising first [S §9.1.6]. Vibrato LFO →
  pitch only; mod LFO → pitch, cutoff, volume.
- **Envelopes:** delay (0), attack (rise to 1, the spec says "convex"), hold (1), decay to
  sustain, sustain, release to 0 [S §9.1.7]. The **volume** envelope works in dB: decay
  and release are linear in dB (so exponential in amplitude), with 96 dB treated as
  silence [S]. The **mod** envelope is linear in its 0…1 value [S]. **[PH]:** attack is
  linear in amplitude for both (the common reading; "convex" is noted as approximated).
- Release time is the time for the full range (96 dB / 1.0), so from a lower sustain level
  release finishes proportionally sooner [I].
- **keynumTo…Hold/Decay:** time (tc) += value × (60 − key) [I: centred on key 60].
- Effects sends: reverb and chorus send levels are kept as data; M3 maps them to nothing
  and reports them (approximated) [PH].
- `exclusiveClass`: a note-on that starts a voice with class c (non-zero) stops voices of
  class c in the same preset quickly (choke) [S].

---

## 11. Unknowns and player differences

1. **Attenuation 0.4 factor** (8.4): a player convention, not in the spec.
2. **Concave curve formula** (8.1): stated loosely by the spec.
3. **SF3 sample-rate and offset generators** (9): the wiki does not say.
4. **SF3 end byte**: "last byte" read as inclusive.
5. **Stereo pair joining** (6.2): our choice.
6. **Envelope attack shape**: "convex" in the spec, linear in practice [I].
7. **ifil minor versions** other than 0, 1, 4: none known.

---

## 12. Fixture plan (generated, never copied)

A small SF2 writer in the importer's test code (`build::Sf2Builder`, like `FlpBuilder` in
M2) builds every fixture from soundgen tones. No third-party SoundFont is committed.

| Id | Content | Exercises |
|---|---|---|
| F-SF2-01 minimal | 1 preset, 1 instrument, 1 mono sample, all defaults | RIFF, INFO, hydra indices, terminal records |
| F-SF2-02 drum kit | bank 128 preset with 8 instrument zones on single keys, scaleTuning 0, exclusiveClass for two hi-hats | percussion bank, choke (acceptance test kit) |
| F-SF2-03 global zones | preset and instrument global zones, local overrides, preset-level relative values, intersecting key/vel ranges | summing rules (section 7) |
| F-SF2-04 attenuation | initialAttenuation 0/240/960 cB, velocity layers via velRange, explicit modulator disabling default #1 | attenuation scale, modulator override |
| F-SF2-05 loops | sampleModes 0, 1, 3; address and coarse offset generators; loop at sample edges | addresses, loops |
| F-SF2-06 pitch | overridingRootKey, original pitch 255, pitch correction −4, coarse/fine tune, keynum and velocity substitution | pitch formula |
| F-SF2-07 envelopes | vol and mod env stages incl. keynumTo… and sustain values; −32768 timecents | envelopes |
| F-SF2-08 lfo/filter | mod LFO to pitch/filter/volume, vib LFO, Fc and Q, modEnvToFilterFc | LFOs, filter |
| F-SF2-09 modulators | CC1→vibrato default, custom CC74→Fc with each curve type, bipolar/direction bits, amount source, abs transform, a linked pair, an illegal CC index | modulator engine |
| F-SF2-10 stereo | left/right linked pair in matching zones; an unmatched linked sample | stereo join |
| F-SF2-11 24-bit | `ifil` 2.04 with `sm24`; same file with `ifil` 2.01 (sm24 must be ignored); bad sm24 size | sm24 rules |
| F-SF2-12 malformed | wrong record sizes, non-monotonic bags, keyRange not first, zone without terminal generator, unknown generator id 70, ROM sample with and without `irom`, duplicate preset numbers | rejection and ignore rules (fuzz seeds) |
| F-SF3-01 vorbis | `ifil` 3, two PCM samples then two Vorbis streams (soundgen tones encoded by the test with the Ogg encoder used by `ph-formats` tests, or a tiny pre-generated stream built in-repo), loops relative to the decoded sample, odd-length `smpl` without pad | SF3 offsets, padding, mixed PCM |
| F-SF3-02 invalid | a stereo Vorbis stream, garbage stream, Opus stream header | SF3 rejection paths |

If `ph-formats` has no Vorbis **encoder**, the SF3 fixtures embed a short Vorbis stream
generated once by a documented command from a soundgen tone and committed with its
provenance (our own audio, so redistributable) [PH].
