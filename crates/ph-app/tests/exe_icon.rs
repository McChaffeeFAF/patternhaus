//! The `.exe` carries the application icon as a resource (M1 follow-up, M2/T10), so Explorer
//! and shortcuts show it before the app runs. Windows only: other platforms set the window
//! icon at runtime and package their own icons.

#[cfg(windows)]
#[test]
fn the_exe_embeds_every_image_of_the_icon() {
    let ico = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packaging/icons/patternhaus.ico"
    ))
    .unwrap();
    let exe = std::fs::read(env!("CARGO_BIN_EXE_ph-app")).unwrap();
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]) as usize;
    let u32_at =
        |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    let images = u16_at(4);
    assert!(images >= 1);
    for k in 0..images {
        let entry = 6 + 16 * k;
        let (size, offset) = (u32_at(entry + 8), u32_at(entry + 12));
        let image = &ico[offset..offset + size];
        // a resource holds each image's bytes unchanged
        let probe = &image[..image.len().min(512)];
        assert!(
            exe.windows(probe.len()).any(|w| w == probe),
            "icon image {k} ({size} bytes) is not in the exe"
        );
    }
}
