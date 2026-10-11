//! The application icon: the Patternhaus mark (see [`crate::brand`]) rasterised at any size by
//! a small supersampling renderer, so the window icon needs no image files and the shipped
//! `.ico` and PNGs (packaging/icons, written by the `app_icons` test) are the same picture.

use crate::theme::Theme;

/// Pixels per side at which the shipped PNGs and the .ico entries are written.
pub const SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

type Rgba = [f32; 4];

fn blend(under: Rgba, over: Rgba) -> Rgba {
    let a = over[3] + under[3] * (1.0 - over[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mix = |u: f32, o: f32| (o * over[3] + u * under[3] * (1.0 - over[3])) / a;
    [
        mix(under[0], over[0]),
        mix(under[1], over[1]),
        mix(under[2], over[2]),
        a,
    ]
}

fn colour(c: egui::Color32, a: f32) -> Rgba {
    [
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        a,
    ]
}

/// Signed distance from `p` to a rounded rectangle centred at `c` with half-size `h`.
fn rrect(p: (f32, f32), c: (f32, f32), h: (f32, f32), r: f32) -> f32 {
    let q = ((p.0 - c.0).abs() - (h.0 - r), (p.1 - c.1).abs() - (h.1 - r));
    let outside = (q.0.max(0.0).powi(2) + q.1.max(0.0).powi(2)).sqrt();
    outside + q.0.max(q.1).min(0.0) - r
}

/// The colour of the icon at a point, in 0..1 unit-square coordinates.
fn sample(theme: &Theme, x: f32, y: f32) -> Rgba {
    let c = &theme.colors;
    let mut px: Rgba = [0.0; 4];
    // the tile
    let d_tile = rrect((x, y), (0.5, 0.5), (0.5, 0.5), 0.2);
    if d_tile > 0.0 {
        return px;
    }
    px = blend(px, colour(c.bg0, 1.0));
    // a faint top light
    let k = (1.0 - y).clamp(0.0, 1.0);
    px = blend(px, colour(c.bg3, 0.35 * k * k));
    // the pad grid
    let pad = 0.16;
    let gap = 0.055;
    let grid = pad * 4.0 + gap * 3.0;
    let origin = 0.5 - grid / 2.0;
    let mut lit_glow = 0.0f32;
    for row in 0..4 {
        for col in 0..4 {
            if crate::brand::lit(row, col) {
                let cx = origin + col as f32 * (pad + gap) + pad / 2.0;
                let cy = origin + row as f32 * (pad + gap) + pad / 2.0;
                let d = rrect((x, y), (cx, cy), (pad / 2.0, pad / 2.0), pad * 0.22);
                lit_glow = lit_glow.max((1.0 - d.max(0.0) / 0.07).clamp(0.0, 1.0).powi(2) * 0.4);
            }
        }
    }
    px = blend(px, colour(c.accent, lit_glow));
    for row in 0..4 {
        for col in 0..4 {
            let cx = origin + col as f32 * (pad + gap) + pad / 2.0;
            let cy = origin + row as f32 * (pad + gap) + pad / 2.0;
            let d = rrect((x, y), (cx, cy), (pad / 2.0, pad / 2.0), pad * 0.22);
            if d <= 0.0 {
                let fill = if crate::brand::lit(row, col) {
                    c.accent
                } else if col / 2 % 2 == 0 {
                    c.pad
                } else {
                    c.pad_group
                };
                px = blend(px, colour(fill, 1.0));
            }
        }
    }
    // the rim
    let rim = (-d_tile / 0.035).clamp(0.0, 1.0);
    px = blend(px, colour(c.accent_dim, 1.0 - rim));
    px
}

/// The icon as `size * size` RGBA bytes (straight alpha), 4 x 4 supersampled.
pub fn rgba(size: u32) -> Vec<u8> {
    let theme = Theme::neon_slate();
    let n = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            let mut weight = 0.0;
            for sy in 0..n {
                for sx in 0..n {
                    let x = (px as f32 + (sx as f32 + 0.5) / n as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / n as f32) / size as f32;
                    let s = sample(&theme, x, y);
                    // average premultiplied, un-premultiply at the end
                    acc[0] += s[0] * s[3];
                    acc[1] += s[1] * s[3];
                    acc[2] += s[2] * s[3];
                    acc[3] += s[3];
                    weight += 1.0;
                }
            }
            let a = acc[3] / weight;
            let un = |v: f32| if acc[3] > 0.0 { v / acc[3] } else { 0.0 };
            for v in [un(acc[0]), un(acc[1]), un(acc[2])] {
                out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            out.push((a.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

/// The window icon for `egui::ViewportBuilder::with_icon`.
pub fn viewport_icon() -> egui::IconData {
    egui::IconData {
        rgba: rgba(256),
        width: 256,
        height: 256,
    }
}

/// A Windows `.ico` holding PNG-compressed entries (`(size, png bytes)`), as Vista and later
/// read them.
pub fn ico(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len() as u32;
    for (size, png) in entries {
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(0); // palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in entries {
        out.extend_from_slice(png);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_tile_with_transparent_corners() {
        let px = rgba(64);
        let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "the corner is transparent");
        assert_eq!(at(32, 2)[3], 255, "the edge middle is opaque");
        // the centre of a lit pad is the accent
        let lit_at = (0..4)
            .flat_map(|r| (0..4).map(move |c| (r, c)))
            .find(|&(r, c)| crate::brand::lit(r, c))
            .unwrap();
        let (pad, gap) = (0.16, 0.055);
        let origin = 0.5 - (pad * 4.0 + gap * 3.0) / 2.0;
        let cx = ((origin + lit_at.1 as f32 * (pad + gap) + pad / 2.0) * 64.0) as usize;
        let cy = ((origin + lit_at.0 as f32 * (pad + gap) + pad / 2.0) * 64.0) as usize;
        let accent = Theme::neon_slate().colors.accent;
        assert_eq!(&at(cx, cy)[..3], &[accent.r(), accent.g(), accent.b()]);
    }

    #[test]
    fn the_ico_container_lists_every_entry() {
        let e = ico(&[(16, vec![1, 2, 3]), (256, vec![4, 5])]);
        assert_eq!(&e[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(e[6], 16, "width of the first entry");
        assert_eq!(e[6 + 16], 0, "256 is stored as 0");
        assert_eq!(e.len(), 6 + 32 + 5);
        // the first entry's data offset points after the directory
        assert_eq!(u32::from_le_bytes([e[18], e[19], e[20], e[21]]), 38);
    }

    /// Writes (with `PH_WRITE_ICONS=1`) or checks the shipped icons in `packaging/icons`.
    #[test]
    fn app_icons_match_the_renderer() {
        use image::ImageEncoder;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons");
        let write = std::env::var_os("PH_WRITE_ICONS").is_some();
        let mut entries = Vec::new();
        for size in SIZES {
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&rgba(size), size, size, image::ExtendedColorType::Rgba8)
                .unwrap();
            let path = dir.join(format!("patternhaus-{size}.png"));
            if write {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &png).unwrap();
            } else {
                let on_disk = std::fs::read(&path)
                    .unwrap_or_else(|_| panic!("{} is missing (PH_WRITE_ICONS=1)", path.display()));
                let a = image::load_from_memory(&on_disk).unwrap().to_rgba8();
                assert_eq!(a.as_raw(), &rgba(size), "{} is stale", path.display());
            }
            entries.push((size, png));
        }
        let ico_bytes = ico(&entries);
        let path = dir.join("patternhaus.ico");
        if write {
            std::fs::write(&path, &ico_bytes).unwrap();
        } else {
            assert!(path.exists(), "{} is missing", path.display());
            let on_disk = std::fs::read(&path).unwrap();
            assert_eq!(&on_disk[..4], &ico_bytes[..4]);
            assert_eq!(on_disk.len(), ico_bytes.len());
        }
    }
}
