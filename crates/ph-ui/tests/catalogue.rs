//! The plugin instrument catalogue in the UI (M3/T9), offscreen: the browser's Catalogue tab
//! lists entries from a catalogue folder, a click shows the playing guide, the guide editor
//! writes through `catalogue.update_guide`, and adding an entry whose plugin is not installed
//! is refused with a plain message. No plugin is loaded, no window or device is opened.

mod common;

use common::*;
use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ph_core::Project;
use ph_ui::PhApp;
use ph_ui::browser_model::Kind;
use ph_ui::core::{Dialog, ToastKind};

const ENTRY: &str = r#"name = "Strings"
tags = ["warm"]

[plugin]
format = "clap"
id = "dev.patternhaus.test.synth"
name = "PH Test Synth"
version = "1.0.0"

[guide]
range = [48, 72]
"#;

#[test]
fn catalogue_tab_lists_shows_the_guide_and_edits_it() {
    let dir = tempfile::tempdir().unwrap();
    let cat = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cat.path().join("strings")).unwrap();
    std::fs::write(cat.path().join("strings/entry.toml"), ENTRY).unwrap();
    std::fs::write(cat.path().join("strings/plugin.state"), b"opaque").unwrap();

    let st = studio(dir.path(), Project::empty("Catalogue"));
    st.set_catalogue_dir(Some(cat.path().to_path_buf()));
    let ctx = egui::Context::default();
    let mut app = PhApp::new(
        &ctx,
        st,
        ph_ui::AppPaths {
            project_dir: dir.path().to_path_buf(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: None,
        },
        ph_ui::Hooks::default(),
        ph_engine::EngineConfig::default(),
        None,
    )
    .expect("app starts");
    app.core.browser.tab = ph_ui::panels::browser::CATALOGUE_TAB;
    let mut h: Harness<'static, PhApp> = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    for _ in 0..3 {
        h.step();
    }
    // nothing is scanned in this session, so the plugin is reported missing
    assert!(
        h.query_all_by_label("Catalogue").count() >= 1,
        "the Catalogue tab shows"
    );
    h.get_by_label("Strings  · warm  (missing)").click();
    h.step();
    let guide = h.state().core.catalogue.guide.clone().unwrap();
    assert!(guide.starts_with("plays C3-C5 (48-72)"), "{guide}");
    assert!(guide.contains("not installed"), "{guide}");

    // the guide editor
    ph_ui::catalogue_ui::open_guide(&mut h.state_mut().core, "strings");
    h.step();
    match &mut h.state_mut().core.dialog {
        Some(Dialog::CatalogueGuide(f)) => {
            assert_eq!(f.guide.range, Some([48, 72]));
            f.guide.mono = true;
            f.guide.keyswitches.push(ph_commands::catalogue::Keyswitch {
                key: 24,
                articulation: "staccato".into(),
            });
        }
        other => panic!("guide editor not open: {other:?}"),
    }
    h.step();
    h.get_by_label("Save guide").click();
    for _ in 0..3 {
        h.step();
    }
    settle(h.state_mut());
    assert!(h.state().core.dialog.is_none());
    let text = std::fs::read_to_string(cat.path().join("strings/entry.toml")).unwrap();
    assert!(text.contains("mono = true"), "{text}");
    assert!(text.contains("articulation = \"staccato\""), "{text}");

    // adding it (double-click or a drop on the rack) is refused plainly: no plugin
    let before = h.state().core.project.channels.len();
    h.state_mut().core.add_channel_from(
        "Strings",
        &Kind::Catalogue {
            entry: "strings".into(),
        },
    );
    settle(h.state_mut());
    let toasts: Vec<_> = h
        .state()
        .core
        .toasts
        .iter()
        .map(|t| (t.kind, t.text.clone()))
        .collect();
    assert!(
        toasts
            .iter()
            .any(|(k, t)| *k != ToastKind::Info && t.contains("not installed")),
        "{toasts:?}"
    );
    assert_eq!(h.state().core.project.channels.len(), before);
}
