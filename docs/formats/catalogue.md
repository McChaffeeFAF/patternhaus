# Plugin instrument catalogue (M3/T9)

The catalogue keeps plugin instruments the owner has set up in the plugin's own editor (for
example a sampled library in its player plugin), so anyone, agents included, can add them to a
project and know how to play them. It is **per user**, outside every project and outside the
repo. It is generic: any CLAP or VST3 instrument works the same way.

Plugin states are **opaque**. Patternhaus saves the bytes the plugin hands it and gives them
back; it never parses, edits or converts them.

## Where it lives

`<user data dir>/Patternhaus/catalogue/`:

| Platform | Default folder |
|---|---|
| Windows | `%APPDATA%\Patternhaus\catalogue` |
| macOS | `~/Library/Application Support/Patternhaus/catalogue` |
| Linux | `$XDG_DATA_HOME/Patternhaus/catalogue` (default `~/.local/share/...`) |

Set `PATTERNHAUS_CATALOGUE` to use another folder. The app and `patternhaus-cli` use this
folder (`ph_control::default_catalogue_dir`, `Studio::set_catalogue_dir`). A session without one
(tests, embedders that do not opt in) refuses the `catalogue.*` commands. Nothing is created
until the first entry is saved.

## Layout

```text
catalogue/
  <entry id>/
    entry.toml     the entry (plain TOML, hand-editable)
    plugin.state   the plugin's saved state (opaque)
```

- **Entry id:** the folder name. It is made from the name when the entry is saved (lowercase
  ASCII letters, digits and `-`, numbered `-2`, `-3` when taken) and never changes; renaming an
  entry changes `name` only. Ids are what the commands take.
- **`plugin.state`** is exactly what a project's `plugins/<file>.state` holds (DESIGN §10): the
  line `PATTERNHAUS-PLUGIN-STATE 1`, one JSON line naming the plugin (`format`, `plugin_id`,
  `version`, `bytes`), then the plugin's own state bytes, unchanged.
- A folder whose `entry.toml` cannot be read is listed as unreadable (with the reason) by
  `catalogue.list`, never dropped silently, and can still be removed.

## `entry.toml`

```toml
name = "Warm Strings (legato)"
description = "Soft section strings; good for pads under a lead."
tags = ["strings", "orchestral", "library name", "vendor name"]

[plugin]
format = "vst3"          # or "clap"
id = "..."               # CLAP id or VST3 class id
name = "..."             # the plugin's name when saved
vendor = "..."
version = "7.10.1"       # the plugin's version when saved

[guide]
range = [28, 96]         # lowest and highest playable key (MIDI, 60 = C4), inclusive
velocity = "two layers, switching at 90; very soft below 30"
mono = false
legato = true            # overlapping notes play legato transitions
notes = "anything else a player should know"

[[guide.keyswitches]]
key = 24                 # C1
articulation = "sustain"

[[guide.keyswitches]]
key = 25
articulation = "staccato"

[[guide.ccs]]
cc = 1
meaning = "dynamics (crossfades layers)"

[[guide.ccs]]
cc = 11
meaning = "expression"
```

Every key except `name` and the plugin's `format` and `id` may be left out. Unknown keys are an
error (so a misspelt key is reported, not ignored).

## In a project

`catalogue.add_to_channel` creates an ordinary plugin channel (`InstrumentSpec::Plugin`) and
gives the project its own copies, so the project never depends on the catalogue:

- `plugins/<name>.state`: a copy of `plugin.state` (saved again from the running plugin on
  `file.save`, like any plugin);
- `plugins/<name>.catalogue.toml`: the entry id, name, plugin and guide at the time it was
  added (`ProjectGuide`). `get_state { view: "plugins" }` shows it for that channel, and
  `file.save_as` copies it along with the state.

## Robustness

- `catalogue.list` and `catalogue.get` report each entry's plugin `status`: `ok`, `missing`
  (not installed, or not scanned yet), `isolate` or `blocked`, with a plain `warning`.
- An entry whose plugin is missing cannot be added or probed; the error says why.
- An entry saved with another plugin version is restored anyway, with a warning naming both
  versions (plugins migrate their own states).
- Restoring happens where every plugin loads: in the render plan, built off the audio thread.

## Probing (`catalogue.probe`)

Renders short notes through the instrument offline and headless (the editor never opens), in
one render so the plugin loads once: a chromatic sweep `low..=high` at each velocity, then, when
candidate keyswitch keys are given, a test note on its own and after each candidate. Each note
is followed by a gap for its release. `ph_analysis::probe` measures each note while it is held:
a note *sounds* when its RMS is above -60 dBFS and at least 6 dB above the tail of the previous
note (measured at the end of the gap). A candidate is proposed as a keyswitch when it is silent
itself and the test note after it sounds different (level by more than 1.5 dB or spectral
centroid by more than 8 %) from the one before.

The result is a **proposal** (range, silent key runs, keyswitches with placeholder names);
the agent or owner checks it and saves it with `catalogue.update_guide`.
