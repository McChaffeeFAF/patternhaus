//! Help > About: version, licence, and third-party notices (DESIGN §11, §13).
//!
//! Default builds are MIT OR Apache-2.0. A build with the `asio` feature links Steinberg's
//! ASIO SDK, so that binary is distributed under GPLv3, and the About box says so.

use crate::theme::Theme;

/// The licence paragraphs of the About box.
pub fn licence_text(asio: bool) -> Vec<String> {
    let mut out = vec![
        format!("Patternhaus {}", env!("CARGO_PKG_VERSION")),
        "Copyright the Patternhaus contributors. Source: https://github.com/McChaffeeFAF/patternhaus"
            .into(),
        "Patternhaus is licensed under the MIT licence or the Apache License 2.0, at your option \
         (LICENSE-MIT, LICENSE-APACHE)."
            .into(),
    ];
    if asio {
        out.push(
            "This build includes ASIO support. ASIO is a trademark and software of Steinberg Media \
             Technologies GmbH. Because it is built with the ASIO SDK under the GPL, this binary \
             as a whole is distributed under the GNU General Public License, version 3 (GPLv3). \
             The Patternhaus source code itself remains MIT OR Apache-2.0; builds without the \
             asio feature are not GPL."
                .into(),
        );
    }
    out.push(
 "Third-party components and their licences, including the bundled Barlow, Barlow Condensed and Share Tech Mono fonts (SIL Open Font Licence 1.1) and the symphonia decoders (MPL-2.0, unmodified), are listed in THIRD_PARTY_NOTICES.md, shipped next to this program."
            .into(),
    );
    out.push(
        "FL Studio is a trademark of Image-Line. Patternhaus is not affiliated with Image-Line; \
         it is an independent, clean-room project."
            .into(),
    );
    out
}

/// True when this binary was built with ASIO (the GPLv3 build).
pub fn asio_build() -> bool {
    ph_engine::ASIO_BUILD
}

/// Draws the About window in the Patternhaus look. Returns false when the user closes it.
pub fn show(ctx: &egui::Context, theme: &Theme) -> bool {
    let c = &theme.colors;
    let open = !ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut close = false;
    egui::Window::new("About Patternhaus")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(480.0, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(c.bg2)
                .stroke(egui::Stroke::new(1.0, c.accent_dim))
                .corner_radius(theme.metrics.radius_lg as u8)
                .inner_margin(egui::Margin::same(20))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: c.glow_soft,
                }),
        )
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 150.0),
                egui::Sense::hover(),
            );
            crate::brand::paint_logo(
                ui.painter(),
                rect.center() + egui::vec2(0.0, 14.0),
                0.9,
                theme,
                1.0,
            );
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("VERSION {}", env!("CARGO_PKG_VERSION")))
                        .font(egui::FontId::new(12.0, crate::fonts::mono()))
                        .color(c.accent),
                );
            });
            ui.add_space(10.0);
            for p in licence_text(asio_build()).iter().skip(1) {
                ui.label(
                    egui::RichText::new(p)
                        .font(egui::FontId::new(12.5, crate::fonts::body()))
                        .color(c.dim),
                );
                ui.add_space(5.0);
            }
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                if crate::widgets::btn(ui, "Close").clicked() {
                    close = true;
                }
            });
        });
    open && !close
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_is_mit_apache_and_names_the_notices() {
        let t = licence_text(false).join("\n");
        assert!(t.contains("MIT") && t.contains("Apache License 2.0"));
        assert!(t.contains("THIRD_PARTY_NOTICES.md"));
        assert!(t.contains("Open Font") && t.contains("Barlow"));
        assert!(!t.contains("GPLv3"));
    }

    #[test]
    fn asio_build_states_gplv3() {
        let t = licence_text(true).join("\n");
        assert!(t.contains("GPLv3") && t.contains("Steinberg"));
        assert_eq!(asio_build(), cfg!(feature = "asio"));
    }
}
