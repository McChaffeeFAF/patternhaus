# Making and sharing a theme

Patternhaus has two built-in themes, **Neon Slate** (the default) and **Red Line**. A theme is
one TOML file. Everything the app draws reads its colours and sizes from the theme, so a file
can change the whole look. (The old Dark and Light themes are gone: a settings file that still
names them falls back to Neon Slate.)

## Try one in a minute

1. Open the config folder. By default it is `%APPDATA%\patternhaus` on Windows,
   `~/Library/Application Support/patternhaus` on macOS and `~/.config/patternhaus` on Linux
   (the `--config DIR` flag changes it).
2. Make a `themes` folder in it and save this as `themes/mint.toml`:

   ```toml
   name = "Mint"
   base = "neon-slate"

   [colors]
   accent     = "#20e3b2"
   accent_hot = "#9bf5dd"
   accent_ink = "#02150f"
   ```

3. In the app: **View > Theme > Mint**. It switches at once; no restart. The choice is
   remembered.

A user theme only needs the keys it changes; everything else comes from its `base` (a built-in
theme: `neon-slate`, the default, or `red-line`). A file that does not parse is listed in the
Theme menu with the reason instead of being dropped.

To share a theme, send the file. To use one you were sent, drop it into `themes/`.

## The file

```toml
id = "mint"          # optional: the name stored in settings.toml (default: the file name)
name = "Mint"        # shown in the menu
base = "neon-slate"  # the built-in this one starts from
dark = true          # false only if you make a light theme: egui's base visuals follow it

[colors]             # every value is "#rrggbb" or "#rrggbbaa"
[glow]               # alphas of the accent glow tokens
[metrics]            # sizes
```

### Colour tokens

| Key | What it colours |
|---|---|
| `bg0` | the deepest surface: the workspace background, wells, the LCD surround |
| `bg1` | bars and docks: the top bar, the toolbar, the browser |
| `bg2` | a window's body |
| `bg3` | title bars and controls at rest |
| `bg4` | raised: hover, knob caps |
| `line`, `line_soft` | outlines and dividers (strong, quiet) |
| `text` | body text and labels |
| `dim` | secondary text, captions |
| `faint` | disabled things only |
| `accent` | the one loud colour: playhead, active steps, meters, selection, focus |
| `accent_hot` | the hot version of the accent: hits, hover on accent fills |
| `accent_ink` | text on an accent fill |
| `lcd` | the LCD readouts' background |
| `warn`, `clip` | warnings (and the meter's upper band), clipping |
| `agent` | everything an agent did: badges, flashes, tinted pads |
| `pad`, `pad_group` | step pads, in alternate groups of four |
| `knob_track` | the 270 degree track under a knob's value arc |
| `pattern` | a list of five colours (c1 to c5): the first swatches of the channel colour menu |
| `ok`, `error` | semantic extras: soloed, OK, errors |

These are derived from the tokens above unless you set them (all optional): `accent_dim`,
`grid_minor`, `grid_major`, `grid_bar` (the piano roll and playlist grid, finest to bar),
`key_white`, `key_black` (the piano roll's row shading), `row_alt`, `note`, `ghost`,
`playhead`, `selection`.

`[glow]` has `glow`, `glow_soft` and `tint`: the alpha (0 to 1) at which the accent becomes
the glow, soft glow and tint colours. Defaults are 0.55, 0.18 and 0.12. Set a glow to 0 for a
flat look.

### Metrics

| Key | Meaning |
|---|---|
| `density` | `"comfortable"` or `"compact"` (compact halves the spacing scale and trims row heights) |
| `rounding`, `radius_sm`, `radius_md`, `radius_lg` | corner radii in px |
| `spacing` | the base unit of the spacing scale (8; steps are half units, so 4 px multiples) |
| `row_height` | height of a list row in px |
| `scale` | extra UI scale on top of the OS scale (1.0) |
| `font_caption`, `font_label`, `font_body`, `font_title`, `font_readout`, `font_lcd` | type sizes in px |

### Type

The fonts are not part of a theme: they are bundled (Barlow Condensed for labels, Barlow for
what you type, Share Tech Mono for readouts). Labels are uppercased when drawn; the text
itself keeps its case, so search and translation still work.

## Rules of the house

- Keep text readable: the built-in themes keep `text` at 7:1 or better against every surface
  and `dim` at 4.5:1. `faint` is for disabled controls only. The unit tests check both
  built-ins; check yours.
- Use the accent sparingly. In the built-in themes it means "live": the playhead, active steps,
  meters, selection and the focused window's edge.
- Agents are `agent`, always: a theme that makes the agent colour equal to the accent hides
  who did what.

## For contributors

Nothing but `crates/ph-ui/src/theme.rs` and `crates/ph-ui/themes/*.toml` may contain a colour
literal. A test (`no_colour_literals_outside_the_theme`) scans the sources and fails the build
on `Color32::from_rgb(..)`, named colours, hex strings and the like. New widgets read
`theme.colors.*`.
