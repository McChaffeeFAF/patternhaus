//! The bundled typefaces (DESIGN section 9), all SIL OFL 1.1, replacing egui's default fonts.
//!
//! - **Barlow Condensed**: every label, menu, tab, section name and button (uppercased at
//!   render time, see [`crate::theme::caps`]).
//! - **Barlow**: content the user typed (names, hints), kept in the case it was typed.
//! - **Share Tech Mono**: readouts (tempo, the bar/beat/tick LCD, dB values, timings).
//!
//! The files live in `assets/fonts` with provenance entries and licence texts.
//!
//! **CJK fallback** (M1 follow-up, M2/T10): names in Chinese, Japanese or Korean need glyphs
//! none of the bundled faces have. Every family falls back to CJK faces found on the system at
//! runtime ([`cjk_fallbacks`]: Microsoft YaHei / Yu Gothic / Malgun Gothic on Windows, PingFang /
//! Hiragino / Apple SD Gothic Neo on macOS, Noto Sans CJK / WenQuanYi / Droid Sans Fallback on
//! Linux, or the file named by `PATTERNHAUS_CJK_FONT`). They are read from the user's own
//! system and never shipped, so they add nothing to the notices; a bundled OFL subset (Noto
//! Sans CJK) can go in front of them once one is committed with its licence. Without any, CJK
//! text renders as replacement boxes, as before.

use egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
    // CJK: the system's faces, after every family's own (so Latin text never changes)
    for (k, face) in cjk_fallbacks().iter().enumerate() {
        let name = format!("cjk-fallback-{k}");
        d.font_data.insert(name.clone(), face.data.clone());
        for list in d.families.values_mut() {
            list.push(name.clone());
        }
    }
    d
}

/// A CJK face found on this system.
#[derive(Clone)]
pub struct CjkFace {
    /// The font file it came from.
    pub path: PathBuf,
    data: Arc<FontData>,
}

/// Candidate CJK fonts, best first: (file, face index in a collection). Chinese and Japanese
/// faces first, then a Korean one (most CJK faces lack Hangul, or kana, or Han).
fn cjk_candidates() -> Vec<(PathBuf, u32)> {
    let mut v: Vec<(PathBuf, u32)> = Vec::new();
    if let Some(p) = std::env::var_os("PATTERNHAUS_CJK_FONT").filter(|p| !p.is_empty()) {
        v.push((PathBuf::from(p), 0));
    }
    let under = |dir: &Path, files: &[&str]| -> Vec<(PathBuf, u32)> {
        files.iter().map(|f| (dir.join(f), 0)).collect()
    };
    if cfg!(windows) {
        let fonts = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("Fonts");
        v.extend(under(
            &fonts,
            &[
                "msyh.ttc",
                "YuGothR.ttc",
                "msgothic.ttc",
                "simsun.ttc",
                "malgun.ttf",
            ],
        ));
    } else if cfg!(target_os = "macos") {
        v.extend(under(
            Path::new("/System/Library/Fonts"),
            &[
                "PingFang.ttc",
                "Hiragino Sans GB.ttc",
                "AppleSDGothicNeo.ttc",
            ],
        ));
    } else {
        for dir in [
            "/usr/share/fonts/opentype/noto",
            "/usr/share/fonts/noto-cjk",
            "/usr/share/fonts/google-noto-cjk",
            "/usr/share/fonts/OTF",
        ] {
            v.extend(under(Path::new(dir), &["NotoSansCJK-Regular.ttc"]));
        }
        for f in [
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ] {
            v.push((PathBuf::from(f), 0));
        }
    }
    v
}

/// What the CJK faces must draw between them: Han, kana and Hangul.
const CJK_PROBES: [char; 3] = ['\u{957f}', '\u{30c1}', '\u{d55c}'];

/// Does this face draw `c`? (Reads the font's character map.)
fn covers(data: &[u8], index: u32, c: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(data, index).is_ok_and(|f| f.charmap().map(c).is_some())
}

/// The CJK fallback faces of this system, loaded once per process: candidates in order, each
/// kept only when it draws a script (Han, kana, Hangul) the ones before it do not. Empty when
/// none is installed.
pub fn cjk_fallbacks() -> &'static [CjkFace] {
    static FACES: OnceLock<Vec<CjkFace>> = OnceLock::new();
    FACES.get_or_init(|| {
        let mut out: Vec<CjkFace> = Vec::new();
        let covered =
            |out: &[CjkFace], c: char| out.iter().any(|f| covers(&f.data.font, f.data.index, c));
        for (path, index) in cjk_candidates() {
            if CJK_PROBES.iter().all(|c| covered(&out, *c)) {
                break;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if !CJK_PROBES
                .iter()
                .any(|c| !covered(&out, *c) && covers(&bytes, index, *c))
            {
                continue;
            }
            // leaked once per process, so every egui context shares the bytes
            let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            let mut data = FontData::from_static(bytes);
            data.index = index;
            out.push(CjkFace {
                path,
                data: Arc::new(data),
            });
        }
        out
    })
}

/// True when the system's CJK faces draw Han, kana and Hangul.
pub fn cjk_complete() -> bool {
    let f = cjk_fallbacks();
    CJK_PROBES
        .iter()
        .all(|c| f.iter().any(|x| covers(&x.data.font, x.data.index, *c)))
}

const FLAG: &str = "ph_fonts_installed";

/// Installs the fonts. egui applies them at the start of the next frame.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(FLAG), true));
}

/// True when the fonts are already installed on this context. Otherwise installs them and
/// returns false: the caller skips the frame (the fonts only exist from the next one, and
/// laying text out in a family egui does not know yet would panic).
pub fn ensure(ctx: &egui::Context) -> bool {
    if ctx
        .data(|d| d.get_temp::<bool>(egui::Id::new(FLAG)))
        .unwrap_or(false)
    {
        return true;
    }
    install(ctx);
    ctx.request_repaint();
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_faces_come_after_every_familys_own_faces() {
        let d = definitions();
        let n = cjk_fallbacks().len();
        for (family, list) in &d.families {
            assert!(!list[0].starts_with("cjk-"), "{family:?}: {list:?}");
            let tail: Vec<&String> = list.iter().rev().take(n).collect();
            assert!(
                tail.iter().all(|f| f.starts_with("cjk-fallback-")),
                "{family:?}: {list:?}"
            );
        }
        // Windows ships Microsoft YaHei and Malgun Gothic: Han, kana and Hangul are covered
        if cfg!(windows) {
            assert!(
                cjk_complete(),
                "{:?}",
                cjk_fallbacks().iter().map(|f| &f.path).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn cjk_names_have_glyphs_when_the_system_has_the_faces() {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions());
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        // Han, kana, Hangul (no spaces: egui reports whitespace as glyphless)
        let text = "\u{957f}\u{540d}\u{524d}\u{30c1}\u{30e3}\u{30f3}\u{30cd}\u{30eb}\u{d55c}\u{ad6d}\u{c5b4}";
        for family in [body(), body_medium(), cond(), cond_bold(), mono()] {
            let ok =
                ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::new(13.0, family.clone()), text));
            assert_eq!(ok, cjk_complete(), "{family:?}");
        }
    }
}
