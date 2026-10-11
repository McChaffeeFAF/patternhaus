//! Help > About: version, licence, and third-party notices (DESIGN §11, §13).
//!
//! Default builds are MIT OR Apache-2.0. A build with the `asio` feature links Steinberg's
//! ASIO SDK, so that binary is distributed under GPLv3, and the About box says so.

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
        "Third-party components and their licences, including the Hack and Ubuntu fonts \
         (SIL Open Font Licence 1.1, Ubuntu Font Licence 1.0) and the symphonia decoders \
         (MPL-2.0, unmodified), are listed in THIRD_PARTY_NOTICES.md, shipped next to this \
         program."
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

/// Draws the About window. Returns false when the user closes it.
pub fn show(ctx: &egui::Context) -> bool {
    let mut open = true;
    egui::Window::new("About Patternhaus")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            for (i, p) in licence_text(asio_build()).iter().enumerate() {
                if i == 0 {
                    ui.heading(p);
                } else {
                    ui.label(p);
                }
                ui.add_space(4.0);
            }
        });
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_is_mit_apache_and_names_the_notices() {
        let t = licence_text(false).join("\n");
        assert!(t.contains("MIT") && t.contains("Apache License 2.0"));
        assert!(t.contains("THIRD_PARTY_NOTICES.md"));
        assert!(t.contains("Open Font") && t.contains("Ubuntu Font"));
        assert!(!t.contains("GPLv3"));
    }

    #[test]
    fn asio_build_states_gplv3() {
        let t = licence_text(true).join("\n");
        assert!(t.contains("GPLv3") && t.contains("Steinberg"));
        assert_eq!(asio_build(), cfg!(feature = "asio"));
    }
}
