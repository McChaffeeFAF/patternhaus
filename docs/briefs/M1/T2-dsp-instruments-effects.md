# Brief M1/T2: DSP, instruments, effects

**Repo:** `E:\patternhaus`. Work on branch `m1/t2-dsp`.
**Read first:** `AGENTS.md` (especially "The audio thread is sacred" and "How to add a processor"); DESIGN §4 and §7; the `ph-node` crate docs.
**Depends on:** the `ph-node` `Processor` contract on `main`. Don't change it. If it's insufficient, stop and report.

## Goal

The M1 sound sources and effects as real-time-safe `ph_node::Processor`s, plus the DSP primitives they share and sample decoding, each proven by tests.

## Scope

### 1. `ph-dsp`: primitives (no `ph-*` dependencies)

- **Parameter smoothing:** a one-pole smoother and a linear ramp.
- **dB/linear helpers:** dB ↔ linear and MIDI key ↔ Hz conversion.
- **Oscillators:** band-limited saw, square/pulse with PWM, triangle and sine (PolyBLEP or better), noise with a seeded PRNG, and a sub-oscillator.
- **Filters:**
  - a ZDF/TPT state-variable filter (LP, HP, BP, notch);
  - a 4-pole ladder (TPT/ZDF, with drive), stable under fast modulation;
  - RBJ biquads (peaking, shelves, LP and HP) for the EQ.
- **Envelopes and LFOs:** ADSR envelopes with curve options; LFOs (sine, triangle, saw, square, sample-and-hold) with tempo sync.
- **Resampling and interpolation:** linear/cubic for `Quality::Live`; windowed sinc for `Quality::High`.
- **Dynamics and saturation:** an oversampling helper (2×/4× polyphase halfband); an envelope follower (peak/RMS, attack/release); a delay line with fractional read.
- **Voice handling:** a voice allocator with a voice limit and oldest/quietest stealing.

### 2. `ph-instruments`

- **`ph.sampler`:** a one-shot or looped sample.
  - **Parameters:** root key, tune and fine tune, start/end, loop mode (off / forward / ping-pong, using the file loop or start/end), an amp ADSR, a filter (SVF, cutoff, resonance, envelope amount), velocity→amp, pan, polyphony, reverse, and a choke group id.
  - Pitch-shifts by resampling, with interpolation chosen by `Quality`.
  - The sample is resolved through `AssetResolver` in `activate`/`load_state`, never on the audio thread.
- **`ph.synth` (Haus Synth):** DESIGN §7.
  - **Sound sources:** 3 oscillators (wave, octave, semitone, fine, level, PWM) plus sub and noise.
  - **Filters:** 2 filters (ladder and SVF models, LP/HP/BP/notch) in serial or parallel; key tracking; envelope amount.
  - **Modulation:** 3 ADSRs (amp, filter, mod), 2 LFOs, and an 8-slot mod matrix. Sources: LFOs, envelopes, velocity, key, mod wheel, note pan. Destinations: pitch, cutoff, resonance, PWM, levels, pan.
  - **Voicing:** unison (voices, detune, spread), glide (legato/always), mono/poly mode, and voice limit with stealing.
- **Factory:** `pub fn create(spec: &ph_core::InstrumentSpec, assets: &dyn AssetResolver) -> Result<Box<dyn Processor>, String>`. It applies `params` by stable id. `MultiSampler` and `Plugin` return a clear "not available until M2/M3" error.

### 3. `ph-effects`: the 7 M1 effects

| Effect | Spec |
|---|---|
| `ph.fx.eq` | 6-band parametric; shelves, peaks, HP/LP |
| `ph.fx.compressor` | Threshold, ratio, knee, attack, release, makeup, peak/RMS detection, sidechain input port, lookahead 0–5 ms (reported via `latency()`) |
| `ph.fx.limiter` | True-peak-aware brickwall: ceiling, release, 4× oversampled detection, lookahead |
| `ph.fx.reverb` | Algorithmic (FDN or Dattorro): size, decay, damping, pre-delay, width, mix. Reports `tail()` |
| `ph.fx.delay` | Stereo/ping-pong, tempo-synced or ms times, feedback, filter in the loop, mix |
| `ph.fx.filter` | SVF and ladder; LP/HP/BP; cutoff, resonance, drive; LFO with sync |
| `ph.fx.saturator` | Soft/hard/tube/tape curves, drive, tone, mix, 2–4× oversampled (more in `Quality::High`) |

Factory: `pub fn create(spec: &ph_core::EffectSpec) -> Result<Box<dyn Processor>, String>`.

### 4. Sample decoding

Put it in `ph-formats`; T1 creates that crate too. If it doesn't exist yet on your branch, create it with only a `decode` module. Merge conflicts are expected and the orchestrator resolves them.

- **Decoding:** `pub fn decode(bytes or path) -> Result<ph_node::SampleData, ...>` using `symphonia` for WAV, AIFF, FLAC, MP3 and OGG.
- **WAV metadata:** read the `smpl` loop points and root key when present.
- **Resampling:** none at decode time. The sampler resamples at play time.

## Real-time and quality bar

- **No allocation on the audio thread.** Every processor gets a test that runs `process` under `assert_no_alloc` (dev-dependency) for thousands of blocks with random events and parameter changes.
- **Deterministic.** Identical input, params and seed give bit-identical output. Test it.
- **Known-answer DSP tests:**
  - filter frequency response within tolerance;
  - oscillator aliasing below a threshold;
  - compressor gain curve matching its static characteristic;
  - limiter output never above its ceiling (true peak, 4× oversampled measurement);
  - reverb tail decaying as reported;
  - no denormal slowdowns (feed silence after a loud signal and time it).
- **Block-size independence.** Rendering with block 64 must equal rendering with block 512, for every processor. Sample-accurate events make this possible; it's essential for the engine's oracle.
- **Benchmarks:** criterion benches for one synth voice, one sampler voice, and each effect at 48 kHz. Record the numbers in your report; they become the performance baseline.

## Done when

- Every processor above exists, with the tests listed.
- `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` all pass.
- The parity tracker rows for instruments and effects have *Wired* filled.

## Out of scope

- The engine graph, mixer insert processing (T3), factory presets and samples (T6), and UI.

## Report back

Summarise:
- the processor list with parameter counts;
- the DSP design choices: filter topology, anti-aliasing method, reverb algorithm;
- the benchmark numbers;
- anything in the `ph-node` contract that felt wrong.

Commit on your branch with sign-off (`git commit -s`) plus a `Co-Authored-By` line. Don't merge to `main`.
