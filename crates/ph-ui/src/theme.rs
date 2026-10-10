//! Themes: TOML files of colour and metric tokens (DESIGN §9). Two ship built in; users can
//! drop more into their config folder.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::Deserialize;

/// All colours the panels use. Field names are the TOML keys under `[colors]`.
#[derive(Clone, Debug)]
pub struct Colors {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_alt: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub ok: Color32,
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
    pub agent: Color32,
    pub meter_lo: Color32,
    pub meter_mid: Color32,
    pub meter_hi: Color32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub rounding: f32,
    pub spacing: f32,
    pub row_height: f32,
    /// Extra UI scale on top of the OS DPI scale.
    pub scale: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            rounding: 4.0,
            spacing: 6.0,
            row_height: 26.0,
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub colors: Colors,
    pub metrics: Metrics,
}

pub const DARK_TOML: &str = include_str!("../themes/dark.toml");
pub const LIGHT_TOML: &str = include_str!("../themes/light.toml");

#[derive(Deserialize)]
struct Raw {
    name: String,
    #[serde(default = "yes")]
    dark: bool,
    colors: toml::Table,
    #[serde(default)]
    metrics: Metrics,
}

fn yes() -> bool {
    true
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

impl Theme {
    pub fn dark() -> Self {
        Self::from_toml(DARK_TOML).expect("built-in dark theme")
    }

    pub fn light() -> Self {
        Self::from_toml(LIGHT_TOML).expect("built-in light theme")
    }

    /// Parses a theme file. Missing colour tokens fall back to the built-in theme of the same
    /// brightness, so a user theme only needs the colours it changes.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
        let base: Raw = toml::from_str(if raw.dark { DARK_TOML } else { LIGHT_TOML })
            .map_err(|e| e.to_string())?;
        let get = |key: &str| -> Result<Color32, String> {
            let v = raw
                .colors
                .get(key)
                .or_else(|| base.colors.get(key))
                .ok_or_else(|| format!("missing colour {key}"))?;
            parse_hex(
                v.as_str()
                    .ok_or_else(|| format!("colour {key} must be a string"))?,
            )
        };
        let colors = Colors {
            bg: get("bg")?,
            panel: get("panel")?,
            panel_alt: get("panel_alt")?,
            widget: get("widget")?,
            widget_hover: get("widget_hover")?,
            widget_active: get("widget_active")?,
            border: get("border")?,
            text: get("text")?,
            text_dim: get("text_dim")?,
            accent: get("accent")?,
            accent_dim: get("accent_dim")?,
            warn: get("warn")?,
            error: get("error")?,
            ok: get("ok")?,
            grid_minor: get("grid_minor")?,
            grid_major: get("grid_major")?,
            grid_bar: get("grid_bar")?,
            key_white: get("key_white")?,
            key_black: get("key_black")?,
            row_alt: get("row_alt")?,
            note: get("note")?,
            ghost: get("ghost")?,
            playhead: get("playhead")?,
            selection: get("selection")?,
            agent: get("agent")?,
            meter_lo: get("meter_lo")?,
            meter_mid: get("meter_mid")?,
            meter_hi: get("meter_hi")?,
        };
        Ok(Self {
            name: raw.name,
            dark: raw.dark,
            colors,
            metrics: raw.metrics,
        })
    }

    /// Applies the theme to an egui context. Scaling is `OS DPI × metrics.scale`.
    pub fn apply(&self, ctx: &egui::Context) {
        let c = &self.colors;
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let r = CornerRadius::same(self.metrics.rounding as u8);
        v.panel_fill = c.panel;
        v.window_fill = c.panel;
        v.extreme_bg_color = c.bg;
        v.faint_bg_color = c.row_alt;
        v.window_stroke = Stroke::new(1.0, c.border);
        v.override_text_color = Some(c.text);
        v.hyperlink_color = c.accent;
        v.selection.bg_fill = c.accent_dim;
        v.selection.stroke = Stroke::new(1.0, c.accent);
        v.widgets.noninteractive.bg_fill = c.panel;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c.text);
        for (w, fill) in [
            (&mut v.widgets.inactive, c.widget),
            (&mut v.widgets.hovered, c.widget_hover),
            (&mut v.widgets.active, c.widget_active),
            (&mut v.widgets.open, c.widget_hover),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, c.border);
            w.fg_stroke = Stroke::new(1.0, c.text);
            w.corner_radius = r;
        }
        v.widgets.noninteractive.corner_radius = r;
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(self.metrics.spacing, self.metrics.spacing * 0.6);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
        });
        ctx.set_zoom_factor(self.metrics.scale.clamp(0.5, 3.0));
    }
}

/// Loads a user theme file, or a built-in by name (`dark`, `light`).
pub fn load(name_or_path: &str) -> Result<Theme, String> {
    match name_or_path {
        "dark" => Ok(Theme::dark()),
        "light" => Ok(Theme::light()),
        path => {
            Theme::from_toml(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?)
        }
    }
}

/// A colour for a channel/pattern (0xRRGGBB).
pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse() {
        assert!(Theme::dark().dark);
        assert!(!Theme::light().dark);
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
            "name = \"x\"\ndark = true\n[colors]\naccent = \"#010203\"\n[metrics]\nscale = 1.5\n",
        )
        .unwrap();
        assert_eq!(t.colors.accent, Color32::from_rgb(1, 2, 3));
        assert_eq!(t.colors.bg, Theme::dark().colors.bg);
        assert_eq!(t.metrics.scale, 1.5);
    }
}
