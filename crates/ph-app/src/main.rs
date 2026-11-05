//! `ph-app`: the Patternhaus desktop application. The app itself is the library
//! ([`ph_app::run`]); a front end that adds instrument importers calls it the same way.

fn main() -> eframe::Result {
    ph_app::run(ph_app::Options::default())
}
