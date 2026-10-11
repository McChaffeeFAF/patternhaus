//! Themes (DESIGN section 9): TOML files of colour tokens and metrics. Two ship built in
//! (Neon Slate, the default, and Red Line); users drop more into `<config>/themes/`.
//!
//! This module and `themes/*.toml` are the only places a colour is written down. Everything
//! else reads [`Colors`] (a test scans the sources for colour literals).
//!
//! Layout of a theme file: `[colors]` (the approved token table plus semantic extras and
//! optional overrides of the derived colours), `[glow]` (alphas of the accent glow tokens)
//! and `[metrics]` (type sizes, spacing, radii, density). A user theme names a `base` built-in
//! and only needs the keys it changes.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::Deserialize;
use std::path::Path;

/// Every colour the panels use. Field names are the TOML keys under `[colors]`.
#[derive(Clone, Debug)]
pub struct Colors {
    // -- the approved token table --------------------------------------------------------
    /// Deepest: the workspace background and wells.
    pub bg0: Color32,
    /// Bars and docks.
    pub bg1: Color32,
    /// Window body.
    pub bg2: Color32,
    /// Title bars and controls.
    pub bg3: Color32,
    /// Raised surfaces: hover, knob caps.
    pub bg4: Color32,
    pub line: Color32,
    pub line_soft: Color32,
    pub text: Color32,
    pub dim: Color32,
    /// Disabled controls only.
    pub faint: Color32,
    pub accent: Color32,
    pub accent_hot: Color32,
    /// Text on an accent fill.
    pub accent_ink: Color32,
    /// The accent at the `[glow]` alphas.
    pub glow: Color32,
    pub glow_soft: Color32,
    pub tint: Color32,
    pub lcd: Color32,
    pub warn: Color32,
    pub clip: Color32,
    pub agent: Color32,
    pub pad: Color32,
    pub pad_group: Color32,
    pub knob_track: Color32,
    /// Pattern / channel palette c1-c5.
    pub pattern: [Color32; 5],
    // -- semantic extras ------------------------------------------------------------------
    pub ok: Color32,
    pub error: Color32,
    // -- derived from the tokens unless the file overrides them -----------------------------
    pub accent_dim: Color32,
    pub grid_minor: Color32,
    pub grid_major: Color32,
    pub grid_bar: Color32,
    pub key_white: Color32,
    pub key_black: Color32,
    pub row_alt: Color32,
    pub note: Color32,
    pub ghost: Color32,
    pub playhead: Color32,
    pub selection: Color32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub density: Density,
    pub rounding: f32,
    pub radius_sm: f32,
    pub radius_md: f32,
    pub radius_lg: f32,
    /// The base unit of the 4 px spacing scale.
    pub spacing: f32,
    pub row_height: f32,
    /// Extra UI scale on top of the OS DPI scale.
    pub scale: f32,
    pub font_caption: f32,
    pub font_label: f32,
    pub font_body: f32,
    pub font_title: f32,
    pub font_readout: f32,
    pub font_lcd: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            density: Density::Comfortable,
            rounding: 3.0,
            radius_sm: 2.0,
            radius_md: 3.0,
            radius_lg: 6.0,
            spacing: 8.0,
            row_height: 26.0,
            scale: 1.0,
            font_caption: 10.0,
            font_label: 11.5,
            font_body: 13.0,
            font_title: 12.5,
            font_readout: 13.0,
            font_lcd: 24.0,
        }
    }
}

impl Metrics {
    /// Spacing multiplier of the density: compact halves the scale.
    fn density_k(&self) -> f32 {
        match self.density {
            Density::Comfortable => 1.0,
            Density::Compact => 0.5,
        }
    }
    /// The spacing scale: `space(1)` is half the base unit (4 px at the default 8, so the
    /// scale sits on the 4 px grid), `space(2)` one unit, and so on. Compact density halves it.
    pub fn space(&self, steps: u32) -> f32 {
        (self.spacing * 0.5 * steps as f32 * self.density_k())
            .round()
            .max(0.0)
    }
    /// Row height after density.
    pub fn row(&self) -> f32 {
        match self.density {
            Density::Comfortable => self.row_height,
            Density::Compact => (self.row_height * 0.85).round(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    /// File stem / built-in id (what `settings.toml` stores).
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub colors: Colors,
    pub metrics: Metrics,
}

pub const NEON_SLATE_TOML: &str = include_str!("../themes/neon-slate.toml");
pub const RED_LINE_TOML: &str = include_str!("../themes/red-line.toml");
/// The default theme's id.
pub const DEFAULT_THEME: &str = "neon-slate";

/// The built-in themes: id, display name, source.
pub const BUILTIN: &[(&str, &str, &str)] = &[
    ("neon-slate", "Neon Slate", NEON_SLATE_TOML),
    ("red-line", "Red Line", RED_LINE_TOML),
];

fn builtin_source(id: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(i, ..)| *i == id).map(|(.., s)| *s)
}

/// `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Result<Color32, String> {
    let h = s.strip_prefix('#').unwrap_or(s);
    let byte = |i: usize| {
        u8::from_str_radix(h.get(i..i + 2).ok_or("too short")?, 16).map_err(|e| e.to_string())
    };
    match h.len() {
        6 => Ok(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Ok(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => Err(format!("colour {s:?} must be #rrggbb or #rrggbbaa")),
    }
}

/// `a` towards `b`, `t` of the way (opaque result).
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A colour for a channel/pattern (0xRRGGBB).
pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GlowAlpha {
    glow: Option<f32>,
    glow_soft: Option<f32>,
    tint: Option<f32>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "yes")]
    dark: bool,
    #[serde(default)]
    colors: toml::Table,
    #[serde(default)]
    glow: GlowAlpha,
    #[serde(default)]
    metrics: Metrics,
}

fn yes() -> bool {
    true
}

/// Overlays `user` onto `base`, table by table.
fn overlay(base: &mut toml::Table, user: &toml::Table) {
    for (k, v) in user {
        match (base.get_mut(k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(u)) => overlay(b, u),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

impl Theme {
    pub fn neon_slate() -> Self {
        Self::from_toml(NEON_SLATE_TOML).expect("built-in Neon Slate theme")
    }

    pub fn red_line() -> Self {
        Self::from_toml(RED_LINE_TOML).expect("built-in Red Line theme")
    }

    /// The default theme.
    pub fn default_theme() -> Self {
        Self::neon_slate()
    }

    /// Parses a theme file. A theme may name a `base` built-in (default Neon Slate); keys it
    /// does not set come from the base, so a user theme only needs what it changes.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let base_id = table
            .remove("base")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| DEFAULT_THEME.to_string());
        let base_src =
            builtin_source(&base_id).ok_or_else(|| format!("unknown base theme {base_id:?}"))?;
        let mut merged: toml::Table = toml::from_str(base_src).map_err(|e| e.to_string())?;
        overlay(&mut merged, &table);
        // identity is the theme's own, never the base's
        match table.get("id") {
            Some(v) => {
                merged.insert("id".into(), v.clone());
            }
            None => {
                merged.remove("id");
            }
        }
        let raw: Raw = merged.try_into().map_err(|e| e.to_string())?;
        let get = |key: &str| -> Result<Color32, String> {
            let v = raw
                .colors
                .get(key)
                .ok_or_else(|| format!("missing colour {key}"))?;
            parse_hex(
                v.as_str()
                    .ok_or_else(|| format!("colour {key} must be a string"))?,
            )
            .map_err(|e| format!("{key}: {e}"))
        };
        let opt = |key: &str, default: Color32| -> Result<Color32, String> {
            if raw.colors.contains_key(key) {
                get(key)
            } else {
                Ok(default)
            }
        };
        let accent = get("accent")?;
        let (bg0, bg1, bg2, bg3, bg4) = (
            get("bg0")?,
            get("bg1")?,
            get("bg2")?,
            get("bg3")?,
            get("bg4")?,
        );
        let (line, line_soft, text, dim) =
            (get("line")?, get("line_soft")?, get("text")?, get("dim")?);
        let accent_hot = get("accent_hot")?;
        let g = &raw.glow;
        let pattern_arr = raw
            .colors
            .get("pattern")
            .and_then(|v| v.as_array())
            .ok_or("missing colour list pattern")?;
        if pattern_arr.len() != 5 {
            return Err("pattern needs exactly 5 colours (c1-c5)".into());
        }
        let mut pattern = [Color32::BLACK; 5];
        for (i, v) in pattern_arr.iter().enumerate() {
            pattern[i] = parse_hex(v.as_str().ok_or("pattern colours must be strings")?)?;
        }
        let colors = Colors {
            bg0,
            bg1,
            bg2,
            bg3,
            bg4,
            line,
            line_soft,
            text,
            dim,
            faint: get("faint")?,
            accent,
            accent_hot,
            accent_ink: get("accent_ink")?,
            glow: accent.gamma_multiply(g.glow.unwrap_or(0.55)),
            glow_soft: accent.gamma_multiply(g.glow_soft.unwrap_or(0.18)),
            tint: accent.gamma_multiply(g.tint.unwrap_or(0.12)),
            lcd: get("lcd")?,
            warn: get("warn")?,
            clip: get("clip")?,
            agent: get("agent")?,
            pad: get("pad")?,
            pad_group: get("pad_group")?,
            knob_track: get("knob_track")?,
            pattern,
            ok: get("ok")?,
            error: get("error")?,
            accent_dim: opt("accent_dim", lerp(bg2, accent, 0.4))?,
            grid_minor: opt("grid_minor", line_soft)?,
            grid_major: opt("grid_major", line)?,
            grid_bar: opt("grid_bar", lerp(line, dim, 0.45))?,
            key_white: opt("key_white", bg2)?,
            key_black: opt("key_black", bg1)?,
            row_alt: opt("row_alt", bg1)?,
            note: opt("note", accent)?,
            ghost: opt("ghost", dim)?,
            playhead: opt("playhead", accent_hot)?,
            selection: opt("selection", text)?,
        };
        let id = raw
            .id
            .clone()
            .unwrap_or_else(|| raw.name.to_lowercase().replace(' ', "-"));
        Ok(Self {
            id,
            name: raw.name,
            dark: raw.dark,
            colors,
            metrics: raw.metrics,
        })
    }

    /// Applies the theme to an egui context: visuals, text styles, spacing and scale.
    /// Cheap enough to call on every theme switch; the fonts are installed once elsewhere.
    pub fn apply(&self, ctx: &egui::Context) {
        let c = &self.colors;
        let m = &self.metrics;
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let r = CornerRadius::same(m.radius_md as u8);
        v.panel_fill = c.bg1;
        v.window_fill = c.bg2;
        v.extreme_bg_color = c.bg0;
        v.faint_bg_color = c.row_alt;
        v.code_bg_color = c.bg0;
        v.window_stroke = Stroke::new(1.0, c.line);
        v.window_corner_radius = CornerRadius::same(m.radius_lg as u8);
        v.menu_corner_radius = r;
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: c.bg0.gamma_multiply(0.8),
        };
        v.popup_shadow = v.window_shadow;
        v.override_text_color = None;
        v.weak_text_color = Some(c.dim);
        v.hyperlink_color = c.accent;
        v.warn_fg_color = c.warn;
        v.error_fg_color = c.error;
        v.selection.bg_fill = c.accent_dim;
        v.selection.stroke = Stroke::new(1.0, c.accent);
        v.widgets.noninteractive.bg_fill = c.bg2;
        v.widgets.noninteractive.weak_bg_fill = c.bg2;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.line_soft);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c.text);
        for (w, fill, fg, stroke) in [
            (&mut v.widgets.inactive, c.bg3, c.text, c.line),
            (&mut v.widgets.hovered, c.bg4, c.text, c.accent_dim),
            (&mut v.widgets.active, c.bg4, c.accent_hot, c.accent),
            (&mut v.widgets.open, c.bg4, c.text, c.accent_dim),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, stroke);
            w.fg_stroke = Stroke::new(1.0, fg);
            w.corner_radius = r;
        }
        v.widgets.noninteractive.corner_radius = r;
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(m.space(1), m.space(1) * 0.75);
            s.spacing.button_padding = egui::vec2(m.space(2), m.space(1) * 0.5);
            s.spacing.interact_size.y = m.row().max(18.0);
            s.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(16.0, crate::fonts::cond_bold()),
                ),
                (
                    TextStyle::Body,
                    FontId::new(m.font_body, crate::fonts::body()),
                ),
                (
                    TextStyle::Button,
                    FontId::new(m.font_label, crate::fonts::cond()),
                ),
                (
                    TextStyle::Small,
                    FontId::new(m.font_caption, crate::fonts::cond()),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(m.font_readout, FontFamily::Monospace),
                ),
            ]
            .into();
        });
        ctx.set_zoom_factor(m.scale.clamp(0.5, 3.0));
    }
}

/// A theme named by id (built-in) or by file path.
pub fn load(name_or_path: &str) -> Result<Theme, String> {
    load_from(name_or_path, None)
}

/// As [`load`], also looking in `<config>/themes/<id>.toml`.
pub fn load_from(name_or_path: &str, config: Option<&Path>) -> Result<Theme, String> {
    // the old dark/light themes were retired; saved settings may still name them
    let name_or_path = match name_or_path {
        "dark" | "light" => DEFAULT_THEME,
        n => n,
    };
    if let Some(src) = builtin_source(name_or_path) {
        return Theme::from_toml(src);
    }
    if let Some(dir) = config {
        let p = dir.join("themes").join(format!("{name_or_path}.toml"));
        if p.exists() {
            return read_theme(&p);
        }
    }
    read_theme(Path::new(name_or_path))
}

fn read_theme(path: &Path) -> Result<Theme, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut t = Theme::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // a user theme without an id is known by its file name
    let has_id = toml::from_str::<toml::Table>(&text).is_ok_and(|t| t.contains_key("id"));
    if !has_id && let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        t.id = stem.to_string();
    }
    Ok(t)
}

/// Every theme the menu offers: the built-ins, then the user's files (id, display name).
/// Files that do not parse are described in the second return value.
pub fn available(config: Option<&Path>) -> (Vec<(String, String)>, Vec<String>) {
    let mut out: Vec<(String, String)> = BUILTIN
        .iter()
        .map(|(i, n, _)| (i.to_string(), n.to_string()))
        .collect();
    let mut errors = Vec::new();
    if let Some(dir) = config.map(|c| c.join("themes"))
        && let Ok(rd) = std::fs::read_dir(&dir)
    {
        let mut files: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for p in files {
            match read_theme(&p) {
                Ok(t) => {
                    if !out.iter().any(|(i, _)| *i == t.id) {
                        out.push((t.id, t.name));
                    }
                }
                Err(e) => errors.push(e),
            }
        }
    }
    (out, errors)
}

impl Theme {
    /// The swatches the channel colour menu offers: the five pattern colours first, then a
    /// neutral set (0xRRGGBB, the form projects store).
    pub fn swatches(&self) -> Vec<u32> {
        let pack = |c: Color32| ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32;
        let mut v: Vec<u32> = self.colors.pattern.iter().map(|c| pack(*c)).collect();
        v.extend([
            0xe5484d, 0xf76b15, 0xffc53d, 0x46a758, 0x12a594, 0x3e63dd, 0x8e4ec6,
        ]);
        v
    }
}

/// Tracking presets (fractions of the font size): menus +6%, labels +12%, captions +20%.
pub const TRACK_MENU: f32 = 0.06;
pub const TRACK_LABEL: f32 = 0.12;
pub const TRACK_CAPTION: f32 = 0.20;

/// Uppercased label text in the condensed face with +12% tracking. The source string keeps
/// its case (searchable, translatable); only the rendering is uppercased.
pub fn caps(text: &str) -> egui::RichText {
    caps_sized(text, 11.5, TRACK_LABEL)
}

/// As [`caps`] with an explicit size and tracking fraction.
pub fn caps_sized(text: &str, size: f32, track: f32) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .font(FontId::new(size, crate::fonts::cond()))
        .extra_letter_spacing(size * track)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse() {
        let n = Theme::neon_slate();
        assert_eq!(n.id, "neon-slate");
        assert_eq!(n.colors.accent, Color32::from_rgb(0x38, 0xc6, 0xff));
        assert_eq!(n.colors.agent, Color32::from_rgb(0xc0, 0x84, 0xfc));
        let r = Theme::red_line();
        assert_eq!(r.id, "red-line");
        assert_eq!(r.colors.accent, Color32::from_rgb(0xff, 0x3b, 0x5c));
        assert_eq!(r.colors.clip, Color32::from_rgb(0xff, 0xe0, 0x66));
        assert_eq!(r.colors.pattern[1], Color32::from_rgb(0xff, 0x8a, 0x3d));
    }

    #[test]
    fn glow_tokens_follow_the_accent() {
        let n = Theme::neon_slate();
        assert_eq!(n.colors.glow, n.colors.accent.gamma_multiply(0.55));
        assert_eq!(n.colors.glow_soft.a(), (255.0f32 * 0.18).round() as u8);
        assert_eq!(n.colors.tint.a(), (255.0f32 * 0.12).round() as u8);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(
            parse_hex("#ff8000").unwrap(),
            Color32::from_rgb(255, 128, 0)
        );
        assert!(parse_hex("#12").is_err());
        assert!(parse_hex("zzzzzz").is_err());
    }

    #[test]
    fn user_theme_overrides_only_what_it_names() {
        let t = Theme::from_toml(
            "name = \"x\"\nbase = \"red-line\"\n[colors]\naccent = \"#010203\"\n[metrics]\nscale = 1.5\n",
        )
        .unwrap();
        assert_eq!(t.colors.accent, Color32::from_rgb(1, 2, 3));
        // `base` picks the built-in the unset keys come from (see docs/theming.md)
        assert_eq!(t.metrics.scale, 1.5);
        assert_eq!(t.metrics.font_body, 13.0);
        assert_eq!(t.colors.glow, t.colors.accent.gamma_multiply(0.55));
        assert_eq!(t.id, "x");
    }

    #[test]
    fn a_bad_theme_names_the_problem() {
        assert!(Theme::from_toml("name = \"x\"\n[colors]\naccent = \"nope\"").is_err());
        assert!(Theme::from_toml("name = \"x\"\nbase = \"nothing\"").is_err());
        assert!(Theme::from_toml("name = \"x\"\n[colors]\npattern = [\"#000000\"]").is_err());
    }

    #[test]
    fn old_theme_names_map_to_the_default() {
        assert_eq!(load("dark").unwrap().id, "neon-slate");
        assert_eq!(load("light").unwrap().id, "neon-slate");
        assert_eq!(load("red-line").unwrap().id, "red-line");
    }

    #[test]
    fn user_themes_load_from_the_config_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        std::fs::write(
            dir.path().join("themes/mine.toml"),
            "name = \"Mine\"\n[colors]\naccent = \"#112233\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("themes/broken.toml"), "name = ").unwrap();
        let (list, errors) = available(Some(dir.path()));
        assert!(list.iter().any(|(i, n)| i == "mine" && n == "Mine"));
        assert_eq!(list[0].0, "neon-slate");
        assert_eq!(errors.len(), 1);
        let t = load_from("mine", Some(dir.path())).unwrap();
        assert_eq!(t.colors.accent, Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(t.id, "mine");
    }

    #[test]
    fn compact_density_tightens_the_scale() {
        let mut m = Metrics::default();
        let roomy = (m.space(2), m.row());
        m.density = Density::Compact;
        assert!(m.space(2) < roomy.0 && m.row() < roomy.1);
        assert_eq!(roomy.0 % 4.0, 0.0);
    }

    #[test]
    fn caps_uppercases_but_keeps_the_source() {
        assert_eq!(caps("Piano roll").text(), "PIANO ROLL");
    }
}
