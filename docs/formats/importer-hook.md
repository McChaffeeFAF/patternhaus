# The importer hook: adding instrument formats from outside Patternhaus

**Status:** M3/T6 (2026-11-05). Audience: anyone writing an instrument importer that is built
and shipped separately from this repository.

Patternhaus reads SFZ, SF2/SF3, DecentSampler and EXS24 instruments itself
(`crates/ph-sampler-formats`). Any other format can be added **without changing Patternhaus**:
you write an importer in your own crate, build your own copy of the app or CLI that registers
it, and every part of Patternhaus that deals with instrument files picks it up:

- `sampler.import` (the UI, scripts and agents over MCP or JSON-RPC);
- the list of instrument extensions (`Importers::extensions`), which the browser's file filter
  and drop-to-create use;
- the import report (`sampler.report`, `import-report.md`, `get_state { view: zones }`).

After the import nothing depends on your importer: the instrument lives in the project (and
can be saved as a native Haus instrument folder), so the project opens and renders the same
in a Patternhaus that has never heard of your format.

## 1. Design

The importers of a session are a plain value, `ph_sampler_formats::Importers`: the built-ins,
then the ones you register, in that order. There is **no global registry**. The front end
builds the set once and passes it to the studio; the studio's host hands it to commands
(`ph_commands::Host::importers`), and the UI reads it from `Studio::importers` /
`StudioHandle::importers`. Two sessions in one process can have different formats, and tests
register importers without affecting anything else.

```text
your binary ──► Importers::builtin() + register(YourImporter)
            ──► ph_app::run(Options { importers })      (the desktop app)
            ──► ph_cli::run(Options { importers })      (patternhaus-cli, incl. `mcp`)
                  └► Studio::start_with(config, project, importers)
                        └► EngineHost::with_importers  ──►  Host::importers()  ──►  sampler.import
```

Lower-level entry points, for embedding: `Studio::start_with` and
`ph_control::HeadlessOptions { importers, .. }`.

## 2. A minimal importer and binary

```rust
use ph_sampler_formats::{
    ImportError, ImportInput, ImportReport, Imported, ImportedSample, Importer, Importers,
    SampleSource,
};

struct Toy;

impl Importer for Toy {
    fn format(&self) -> &'static str { "toy" }          // unique id, lower case
    fn extensions(&self) -> &'static [&'static str] { &["toy"] }   // lower case, no dot
    fn version(&self) -> u32 { 1 }                      // bump when the mapping changes
    fn import(&self, input: ImportInput<'_>) -> Result<Imported, ImportError> {
        // read input (bytes or a path), build a HausInstrument, list its samples
        todo!()
    }
}

fn main() -> std::process::ExitCode {
    let mut importers = Importers::builtin();
    importers.register(Toy).expect("toy is a new format");
    ph_cli::run(ph_cli::Options { importers })   // or ph_app::run(ph_app::Options { importers })
}
```

Depend on the Patternhaus crates by path or git (`ph-sampler-formats`, `ph-core`, and
`ph-app` and/or `ph-cli`). The stock `ph-app` and `patternhaus-cli` binaries are exactly
these two calls with `Options::default()`, so your binaries behave identically apart from the
extra formats.

`Importers::register` refuses, and leaves the set unchanged, when:
- the format id is empty or not lower-case letters, digits, `-` or `_`;
- the importer declares no extensions, or an extension that is not in that form (no dot, no
  upper case);
- another importer already has that format id or reads one of its extensions. Built-ins always
  keep their extensions: an importer cannot take over `.sfz`.

## 3. The `Importer` contract

`import` receives `ImportInput::Path` (what `sampler.import` passes: you may read the file and
files beside it) or `ImportInput::Bytes { name, bytes }` (tests and fuzzing; relative sample
paths cannot be resolved). It returns `Imported`:

- **`instrument`**: a `ph_core::HausInstrument` with id 0, `folder` unset, `source` filled
  (`format` = your format id, `file`, `importer_version` = `version()`), and its groups and
  zones keyed by their own ids. Zones play `SampleRef::File(name)`, where `name` is a path
  relative to the instrument folder, conventionally `samples/<file>`.
- **`samples`**: one `ImportedSample { name, source }` for every name the zones use. The
  `source` says where the audio is:

  | `SampleSource` | Use for | What `sampler.import` does |
  |---|---|---|
  | `File(path)` | an audio file on disk that `ph-formats` decodes | links it in place when every sample is a plain file under one folder whose relative path is the zone name; otherwise copies it |
  | `Audio(Arc<SampleData>)` | audio you decoded yourself (a compressed or custom codec) | writes it as a float WAV into the project |
  | `Chunk { container, offset, length, encoding, sample_rate, channels }` | a byte range of a container file | cuts it out into the project: `Pcm16Le`, `Pcm24Le`, `F32Le` (interleaved frames) become a float WAV; `EmbeddedFile` and `OggVorbis` ranges are written as they are, so the zone name's extension must match |
  | `Generated(..)` | the SFZ built-in generators | writes the generated WAV |

- **`report`**: `ImportReport` with `format`, `file`, group and zone counts, and the rules
  below.

**Never panic, on any input.** The host runs every importer through
`ph_sampler_formats::import_with`, which catches a panic and turns it into an error naming
your format, but a panic is still a bug. Guard every length, offset and count against the
real size of the data; never allocate a size read from the file without a bound; never
recurse without a depth limit; never index without a check.

**Accepted imports are checked.** `import_with` also runs `check_imported`: the instrument
must pass the model's checks (`validate_instrument_header`, `validate_group`,
`validate_zone`), every zone must be in an existing group, and every `SampleRef::File` name
must be listed in `samples`. A failure is reported as "the <format> importer produced an
invalid instrument". Clamp values to the model's ranges and report the clamping instead.

## 4. Report rules

The person (and the agent) must be able to see what did not come across exactly. Nothing is
dropped silently.

- **Approximated** (`ImportNoteKind::Approximated`): mapped, but not exactly (a filter type
  played as the nearest one, a curve simplified). Say what and how in `detail`.
- **Skipped** (`ImportNoteKind::Skipped`): read and understood, but not carried over (a
  feature the Haus Sampler does not have, a missing sample file).
- **Kept** (`ImportNoteKind::Kept`): data kept for later in `HausInstrument::extra` under
  `<format>.<what>` keys and never interpreted (scripts, UI definitions; M3b).
- **Not understood** (`ImportReport::unknown`): fields, chunks or opcodes you could not
  interpret at all, with counts.
- Use one note per kind of thing, with a `count`, not one note per zone.
- Put the same notes in `instrument.import_notes`, so they travel with the project.
- Wording is for a musician: "loop crossfade shortened to 50 ms", not "field 0x3A clamped".

## 5. Refusal rules

Return an `ImportError`; never a half-built instrument.

- **`Encrypted { format, file, detail }`: encrypted or licence-locked content.** Detect it from
  the documented signals of your format and refuse at the first one. Never decrypt, never
  try to work around a licence check, never read past the signal, and never log or keep the
  protected bytes. The message the person sees is
  "`<file>` is an encrypted or licence-locked `<format>` instrument (`<detail>`); Patternhaus
  does not open protected libraries", so `detail` names the signal in plain words. This is a
  permanent boundary of the project (DESIGN §8).
- **`Unavailable { format, track }`**: a variant of your format this build does not read.
  `sampler.import` reports it as the command error `Unavailable`.
- **`Malformed { format, file, reason }`**: not a valid file of the format.
- **`Unsupported { file, reason }`**: a valid file using something you cannot represent.
- **`Io { path, message }`**: the file system failed.

## 6. Testing and fuzzing

- **Fixtures are generated**, byte by byte from your format notes, by test code. Never
  commit third-party instrument files, samples or presets.
- **Fuzz every entry point** the way `crates/ph-sampler-formats/tests/fuzz.rs` does: bounded
  proptests over random bytes and over mutated copies of your fixtures (replace, insert,
  truncate), run with the normal test suite. Every outcome must be an `Err` or an `Imported`
  that passes `check_imported`; none may panic or allocate without bound. Feed both
  `ImportInput::Bytes` and `ImportInput::Path` (a temp folder with the samples your seeds
  reference), so the path-reading code is fuzzed too.
- Fuzz the refusal paths as well: a file with a protection signal followed by random bytes
  must always be refused, never parsed.
- **End to end:** register your importer in a headless studio
  (`HeadlessOptions { importers, .. }`), import through `sampler.import`, render, save as
  native (`sampler.save_native`), reopen and render again; the two renders must be
  identical. `crates/ph-cli/tests/importer_hook.rs` does exactly this over MCP with a made-up
  text format, and is a template.
