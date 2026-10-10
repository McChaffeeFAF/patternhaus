# Brief M1/T8: Visual pass

**Repo:** `E:\patternhaus`. Work in a worktree on branch `m1/t8-visuals`. It starts after T7 has merged.
**Read first:**
- DESIGN §9, especially **Look: revised 2026-10-10**, which is the spec;
- the approved visual-direction canvas: https://claude.ai/artifact/AJfwGWHTjpk2dH8n8oKf5J (artboards: Neon Slate workspace, Red Line, component kit). Its token values are reproduced below, so you don't need access to the canvas;
- `AGENTS.md`;
- `crates/ph-ui` (`theme.rs`, `widgets`, panels, `workspace.rs`, `windows.rs`, the piano roll and playlist).

**Why:** the owner wants Patternhaus to make a professional first impression. They approved a neon-cyberpunk direction at FL-level density and polish, in our own visual language.

## Approved tokens

| token | Neon Slate | Red Line |
|---|---|---|
| bg0 (deepest) | `#0b1016` | `#110b0e` |
| bg1 (bars, docks) | `#111820` | `#181015` |
| bg2 (window body) | `#16202a` | `#20151b` |
| bg3 (title bars, controls) | `#1e2a36` | `#2a1c24` |
| bg4 (raised, knob caps) | `#263444` | `#36242e` |
| line | `#2a3949` | `#3b2833` |
| lineSoft | `#1c2732` | `#271a21` |
| text | `#dbe7f3` | `#f1dfe4` |
| dim | `#8fa2b8` | `#b6949e` |
| faint (disabled only) | `#5d6f84` | `#7e6069` |
| accent | `#38c6ff` | `#ff3b5c` |
| accentHot | `#9be6ff` | `#ff9aab` |
| accentInk (text on accent) | `#03141d` | `#1d0308` |
| glow / glowSoft / tint | accent at 55% / 18% / 12% | same rule |
| lcd | `#060a0e` | `#0b0608` |
| warn | `#ffb648` | `#ffc247` |
| clip | `#ff3d6a` | `#ffe066` |
| agent | `#c084fc` | `#3fd8ff` |
| pad / padGroup | `#1b2632` / `#223040` | `#24171e` / `#2e1e27` |
| knobTrack | `#2b3a4a` | `#3a2732` |
| pattern colours c1–c5 | `#38c6ff #7c8cff #2dd4bf #94a3b8 #f472b6` | `#ff3b5c #ff8a3d #f2c14e #a8939a #c084fc` |

**Type:**
- **Barlow Condensed** (400/500/600/700): every label, menu, tab, section name and button. Uppercase, with letter-spacing of +6% for menus, +12% for labels and +20% for captions.
- **Barlow** (400/500/600): content the user typed (channel, pattern and preset names, file names, hints). Keep it in the case it was typed.
- **Share Tech Mono:** readouts (tempo, the bar·beat·tick LCD, dB values, timings, insert numbers).
- **Bundling:** all three are OFL. Bundle the font files in `assets/fonts` with provenance entries, and replace egui's default fonts (removing `epaint_default_fonts` and its Ubuntu font licence from the notices).

## Scope

### 1. Design system in code (`ph_ui::theme`)

- **Tokens:** the token table above as the theme model. Theme files (TOML) carry every token, plus metrics (type sizes, spacing scale on a 4 px grid, radii, density: compact or comfortable).
- **Themes:** ship `neon-slate.toml` (default) and `red-line.toml`. The old dark and light themes become user-selectable extras, retinted to the new token set, or are removed. Say which.
- **Live switching:** the theme menu switches themes live. User themes load from the config folder, with a short `docs/theming.md` on how to make and share one.
- **One source:** every widget reads tokens; no hard-coded colours remain (add a test or lint that scans for colour literals outside `theme`).

### 2. Typography and icons

- **Fonts and casing:** the bundled fonts, following the casing rules above. Labels are uppercased at render time; the source strings stay in normal case, so they remain searchable and translatable.
- **Icon set:** one consistent stroke icon set (16 px grid, 1.5 px stroke), drawn as vectors. It covers the transport, tools (draw, paint, delete, mute, slice, select, zoom), snap, loop, metronome, undo/redo, save, export, the window buttons (detach, maximize, close), the browser (folder, sample, preset, plugin) and LEDs.
  - Draw them ourselves, or use a permissive set (Lucide is ISC; Phosphor is MIT) with provenance recorded.
  - Replace text buttons like "Byp" and "Del" with icons plus tooltips.

### 3. Controls, in the approved style

| Control | Spec |
|---|---|
| Knob | 270° track with a value arc in the accent colour and a soft glow; bipolar knobs fill from centre; pointer line; the value readout appears on hover and drag |
| Fader | Recessed track, cap with grip, dB scale ticks |
| Meter | Gradient accent → warn → clip, peak-hold line, clip LED, smooth falloff (in the UI only; the audio path is untouched) |
| Step pad | Recessed; alternate shading per group of 4; velocity sets accent opacity; the playhead column is outlined; a hit flashes accentHot with glow; agent-edited pads tint with the agent colour for 2 s |
| Buttons | Default, hover, pressed, toggle-on (accent fill, accentInk text) and disabled states |
| Tabs | Accent underline on the active tab |
| LEDs | On, off, warn, clip |
| Window title bars | Focused: accent edge plus soft outer glow. Background windows: no glow, dim title |
| LCD | Tempo and bar·beat·tick readouts on the lcd background, accent text with glow |

### 4. Layout density and chrome (match the Neon Slate artboard)

- **Top bar:** wordmark, uppercase menus, PAT/SONG, transport, tempo LCD, the large bar·beat·tick LCD, an oscilloscope, DSP and RAM readouts, the pattern selector, and the agent badge.
- **Toolbar:** tool icons, snap, and a master stereo meter with a dB readout.
- **Browser:** uppercase tabs and a search field; the tree has folder, sample and preset icons; a sample preview waveform sits at the bottom.
- **Mixer strips:** vertical names, stereo meters, fader, pan knob, dB readout, FX count. A side panel shows the FX chain and the sends.
- **Hint bar** at the bottom: hovering any control shows its name, current value and gestures. Each widget registers its hint text, and every widget gets one. Agents can read the current hint through `get_state`; the view addition belongs in `ph-control`.

### 5. Motion

These all run on UI time, never the audio thread, and repaint only while something is animating (no idle CPU burn). Honour a "reduce motion" setting.
- step pads flash on hit;
- meters fall off smoothly and hold their peaks;
- the playhead has a soft trail and glow;
- the focused window's glow fades in over 120 ms;
- knob arcs ease (60–90 ms);
- agent toasts slide in;
- agent-edited notes and pads pulse once.

### 6. Customisability (FL-style)

- **Right-click menus** on every control, with consistent entries (reset, automation placeholder, MIDI-learn placeholder, help).
- **Per-window options menu** in each title bar.
- **Customisable toolbar:** show, hide and reorder items, saved with the layout.

### 7. Piano roll and playlist grid down to 1/64

- **Snap names:** musical — 1/4, 1/8, 1/16, 1/32, 1/64, plus triplets 1/8T, 1/16T, 1/32T, 1/64T, plus Bar, 1/2 bar, Beat and Off. These are 960, 480, 240, 120, 60, then 320, 160, 80, 40 ticks. Keep the old settings readable.
- **Five-tier grid:** bar, beat, 1/16, 1/32, 1/64, each line one step dimmer. Each tier fades in only once it's at least ~6 px apart; beats are shaded alternately.
- **Tiny notes:** a minimum drawn width (3 px) and a minimum click target (8 px) for very short notes; labels only once there's room.
- **Zoom presets:** fit, 1 bar, 1 beat, and 1/64 detail, with keyboard steps that land on them.
- **Tests:** grid tier visibility by zoom, snap at every option, and grabbing a 1/64 note at normal zoom.

### 8. Polish

A splash or loading screen with the wordmark, an app icon (Windows .ico plus PNGs), the window title format, and an About box in the new style.

## Rules

- **No visible windows:** never launch the app. Use offscreen `egui_kittest` for everything. **Never open the real audio device.**
- **Clean room:** match FL's *behaviour*, never its visual design. The look is the approved Patternhaus direction.
- **Performance:**
  - the 10,000-note piano-roll frame stays under 8 ms (CPU layout plus paint);
  - glow is cheap (layered strokes or pre-blurred textures, never per-frame blur passes);
  - re-measure and report.
- **Screenshots:** regenerate all of them in both themes (`docs/screenshots/M1/neon-*.png`, `red-*.png`), including a busy workspace and a 1/64-zoomed piano roll.
- **Docs:** update the manual test script and `docs/theming.md`.
- **Checks:** `cargo fmt`, `clippy -D warnings`, `test` and `xtask check` (including provenance for fonts and icons) all pass.
- **Commits:** commit with `git commit -s` plus a `Co-Authored-By` line. Don't merge to main.

## Report back

Summarise:
- each section and how it was done;
- the token and theme file layout;
- the icon source;
- the performance numbers;
- the screenshot paths;
- any contract changes;
- test counts;
- the commit list.
