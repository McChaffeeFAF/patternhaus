//! The bundled typefaces (DESIGN section 9), all SIL OFL 1.1, replacing egui's default fonts.
//!
//! - **Barlow Condensed**: every label, menu, tab, section name and button (uppercased at
//!   render time, see [`crate::theme::caps`]).
//! - **Barlow**: content the user typed (names, hints), kept in the case it was typed.
//! - **Share Tech Mono**: readouts (tempo, the bar/beat/tick LCD, dB values, timings).
//!
//! The files live in `assets/fonts` with provenance entries and licence texts.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const BARLOW_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Barlow-Regular.ttf");
const BARLOW_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Barlow-Medium.ttf");
const COND_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/BarlowCondensed-Medium.ttf");
const COND_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/BarlowCondensed-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/ShareTechMono-Regular.ttf");

/// Labels, menus, tabs, buttons: Barlow Condensed Medium.
pub fn cond() -> FontFamily {
    FontFamily::Name("cond".into())
}
/// Headings and emphasis in the label face: Barlow Condensed SemiBold.
pub fn cond_bold() -> FontFamily {
    FontFamily::Name("cond-bold".into())
}
/// User content: Barlow.
pub fn body() -> FontFamily {
    FontFamily::Proportional
}
/// User content, emphasised: Barlow Medium.
pub fn body_medium() -> FontFamily {
    FontFamily::Name("body-medium".into())
}
/// Readouts: Share Tech Mono.
pub fn mono() -> FontFamily {
    FontFamily::Monospace
}

/// The font set egui is given.
pub fn definitions() -> FontDefinitions {
    let mut d = FontDefinitions::empty();
    for (name, bytes) in [
        ("Barlow", BARLOW_REGULAR),
        ("Barlow Medium", BARLOW_MEDIUM),
        ("Barlow Condensed", COND_MEDIUM),
        ("Barlow Condensed SemiBold", COND_SEMIBOLD),
        ("Share Tech Mono", MONO),
    ] {
        d.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fam = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    d.families
        .insert(FontFamily::Proportional, fam(&["Barlow"]));
    d.families
        .insert(FontFamily::Monospace, fam(&["Share Tech Mono", "Barlow"]));
    d.families
        .insert(cond(), fam(&["Barlow Condensed", "Barlow"]));
    d.families
        .insert(cond_bold(), fam(&["Barlow Condensed SemiBold", "Barlow"]));
    d.families
        .insert(body_medium(), fam(&["Barlow Medium", "Barlow"]));
    d
}

/// Installs the fonts (once, at start-up).
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}
