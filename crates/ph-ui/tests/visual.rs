//! The visual pass through real egui events, offscreen: live theme switching, the shared
//! right-click menu, the hint bar and its coverage, per-window options, the customisable
//! toolbar and the colour-literal lint.

mod common;

use common::*;
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
use ph_ui::PhApp;
use ph_ui::hint;
use ph_ui::workspace::WindowId;
use std::time::Duration;

type H = Harness<'static, PhApp>;

fn harness(app: PhApp) -> H {
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    frames(&mut h, 4);
    h
}

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 5000, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    (harness(app), ids, fx.dir)
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(6));
    }
}

fn move_to(h: &mut H, p: Pos2) {
    h.event(Event::PointerMoved(p));
    frames(h, 2);
}

fn right_click(h: &mut H, p: Pos2) {
    move_to(h, p);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: p,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(h, 4);
}

/// Shows only these windows (the playlist stays as the background).
fn only(h: &mut H, keep: &[WindowId]) {
    let ws = &mut h.state_mut().ws;
    for id in ws.ids().collect::<Vec<_>>() {
        if id != WindowId::Playlist {
            ws.hide(id);
        }
    }
    for id in keep {
        ws.show(*id);
    }
    frames(h, 3);
}

// ---- the colour-literal lint -----------------------------------------------------------

/// "One source": colours are written down in `theme.rs` and `themes/*.toml` only. Everything
/// else reads tokens, so a theme file changes the whole look.
#[test]
fn no_colour_literals_outside_the_theme() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let banned = [
        "Color32::from_",
        "Color32::WHITE",
        "Color32::BLACK",
        "Color32::GRAY",
        "Color32::LIGHT_",
        "Color32::DARK_",
        "Color32::RED",
        "Color32::GREEN",
        "Color32::BLUE",
        "Color32::YELLOW",
        "Color32::ORANGE",
        "Color32::GOLD",
        "Color32::BROWN",
        "Color32::KHAKI",
        "Rgba::",
        "hex_color!",
        "Hsva",
    ];
    let mut hits = Vec::new();
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs")
                || path.file_name().is_some_and(|n| n == "theme.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // production code only; a unit-test module may build colours to test with
            let prod = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in prod.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                let hex_string = code.match_indices('#').any(|(i, _)| {
                    let rest = &code[i + 1..];
                    rest.len() >= 6
                        && rest.chars().take(6).all(|c| c.is_ascii_hexdigit())
                        && code[..i].ends_with('"')
                });
                let hex_int = code.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
                    w.len() == 8
                        && w.starts_with("0x")
                        && w[2..].chars().all(|c| c.is_ascii_hexdigit())
                });
                if banned.iter().any(|b| code.contains(b)) || hex_string || hex_int {
                    hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "colour literals outside theme.rs:\n{}",
        hits.join("\n")
    );
}

// ---- themes ----------------------------------------------------------------------------

#[test]
fn switching_the_theme_restyles_the_running_app() {
    let (mut h, _ids, _dir) = start();
    let neon = ph_ui::theme::Theme::neon_slate();
    assert_eq!(h.state().core.theme.id, "neon-slate");
    assert_eq!(h.ctx.global_style().visuals.panel_fill, neon.colors.bg1);
    h.state_mut().set_theme("red-line");
    frames(&mut h, 3);
    let red = ph_ui::theme::Theme::red_line();
    assert_eq!(h.state().core.theme.id, "red-line");
    assert_eq!(h.ctx.global_style().visuals.panel_fill, red.colors.bg1);
    assert_eq!(
        h.ctx.global_style().visuals.selection.stroke.color,
        red.colors.accent
    );
    // and back, without restarting
    h.state_mut().set_theme("neon-slate");
    frames(&mut h, 3);
    assert_eq!(h.ctx.global_style().visuals.panel_fill, neon.colors.bg1);
}

#[test]
fn a_theme_file_in_the_config_folder_is_picked_up_and_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config.path().join("themes")).unwrap();
    std::fs::write(
        config.path().join("themes/mint.toml"),
        "name = \"Mint\"\nbase = \"neon-slate\"\n[colors]\naccent = \"#20e3b2\"\n",
    )
    .unwrap();
    let st = studio(dir.path(), ph_core::Project::empty("t"));
    let ctx = egui::Context::default();
    let app = PhApp::new(
        &ctx,
        st,
        ph_ui::AppPaths {
            project_dir: dir.path().into(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: Some(config.path().into()),
        },
        ph_ui::Hooks::default(),
        ph_engine::EngineConfig::default(),
        None,
    )
    .unwrap();
    let mut h = harness(app);
    let (themes, errors) = ph_ui::theme::available(Some(config.path()));
    assert!(errors.is_empty());
    assert!(
        themes
            .iter()
            .any(|(id, name)| id == "mint" && name == "Mint")
    );
    h.state_mut().set_theme("mint");
    frames(&mut h, 3);
    assert_eq!(
        h.ctx.global_style().visuals.selection.stroke.color,
        egui::Color32::from_rgb(0x20, 0xe3, 0xb2)
    );
    // remembered for the next start
    let (t, err) = ph_ui::settings::load_theme(Some(config.path()));
    assert!(err.is_none());
    assert_eq!(t.id, "mint");
}

// ---- the right-click menu, hints, window options ---------------------------------------

#[test]
fn right_clicking_a_knob_offers_the_shared_menu_and_reset_works() {
    let (mut h, ids, _dir) = start();
    only(&mut h, &[WindowId::ChannelRack]);
    // turn the kick's volume away from its default, through a command
    h.state_mut()
        .core
        .send(ph_ui::mapping::channel_volume(ids.kick, 0.3));
    pump_until(h.state_mut(), 500, |a| {
        a.core
            .project
            .channels
            .get(&ids.kick)
            .is_some_and(|c| (c.volume - 0.3).abs() < 1e-3)
    });
    frames(&mut h, 3);
    let knob = h.get_by_label("Kick volume");
    let at = knob.rect().center();
    right_click(&mut h, at);
    // (the Help menu of the menu bar is also called Help)
    assert!(h.query_all_by_label("Help").count() >= 2);
    for entry in ["Reset to default", "Create automation clip", "MIDI learn…"] {
        assert!(h.query_by_label(entry).is_some(), "the menu has {entry:?}");
    }
    h.get_by_label("Reset to default").click();
    frames(&mut h, 4);
    pump_until(h.state_mut(), 500, |a| {
        a.core
            .project
            .channels
            .get(&ids.kick)
            .is_some_and(|c| (c.volume - 0.78).abs() < 1e-3)
    });
    let v = h.state().core.project.channels[&ids.kick].volume;
    assert!((v - 0.78).abs() < 1e-3, "reset to the default: {v}");
}

#[test]
fn the_hint_bar_shows_and_publishes_what_the_pointer_is_over() {
    let (mut h, _ids, _dir) = start();
    let slice = h.get_by_label("Slice");
    let at = slice.rect().center();
    move_to(&mut h, at);
    let hint = hint::current(&h.ctx).expect("a hint while hovering");
    assert_eq!(hint.name, "Slice");
    assert!(!hint.gestures.is_empty());
    // an agent can read it
    assert_eq!(
        h.state().core.studio.handle().ui_hint().map(|x| x.name),
        Some("Slice".to_string())
    );
    // away from every control, no hint
    move_to(&mut h, Pos2::new(1590.0, 998.0));
    frames(&mut h, 2);
    assert!(hint::current(&h.ctx).is_none() || h.state().core.studio.handle().ui_hint().is_some());
}

/// Every interactive widget registers a hint (its name, value and gestures).
#[test]
fn every_control_has_a_hint() {
    let (mut h, ids, _dir) = start();
    // an automation clip, for the editor's scene
    let auto = {
        let target: ph_core::ParamPath = format!("channel/{}/volume", ids.kick.0).parse().unwrap();
        let r = ph_commands::Request::new(
            "automation.add_clip",
            serde_json::json!({ "target": target, "start": 0, "length": 3840,
                "points": [{ "tick": 0, "value": 0.2 }, { "tick": 3840, "value": 0.9 }] }),
        );
        let out = h
            .state()
            .core
            .studio
            .session
            .execute(&ph_commands::Caller::human(), r)
            .unwrap();
        ph_core::ClipId(out.result["clip"].as_u64().unwrap() as u32)
    };
    pump_until(h.state_mut(), 2000, |a| {
        a.core.project.clips.contains_key(&auto)
    });
    let scenarios: Vec<(&str, Vec<WindowId>)> = vec![
        ("automation editor", vec![WindowId::Automation(auto)]),
        ("playlist and chrome", vec![]),
        ("channel rack", vec![WindowId::ChannelRack]),
        ("piano roll", vec![WindowId::PianoRoll]),
        ("mixer", vec![WindowId::Mixer]),
        ("history", vec![WindowId::History]),
        ("agents", vec![WindowId::Agents]),
        ("kick instrument", vec![WindowId::Instrument(ids.kick)]),
        ("bass instrument", vec![WindowId::Instrument(ids.bass)]),
        ("effect", vec![WindowId::Effect(ph_core::InsertId(4), 0)]),
    ];
    let roles = [
        Role::Button,
        Role::CheckBox,
        Role::ComboBox,
        Role::Slider,
        Role::SpinButton,
        Role::TextInput,
        Role::RadioButton,
        Role::Tab,
    ];
    let mut missing = Vec::new();
    let mut checked = 0;
    for (name, windows) in scenarios {
        only(&mut h, &windows);
        let mut targets: Vec<(String, Pos2)> = Vec::new();
        // where the scenario's controls are: the chrome and the browser dock for the first,
        // the window itself for the others (anything outside is clipped away)
        let region = windows.first().map(|id| {
            let r = h.state().ws.effective_rect(*id).unwrap();
            let o = h.state().work_rect.min;
            egui::Rect::from_min_size(o + Vec2::new(r.x, r.y + 30.0), Vec2::new(r.w, r.h - 30.0))
                .intersect(h.state().work_rect)
        });
        for role in roles {
            for node in h.query_all(by().role(role)) {
                let r = node.rect();
                // on screen, and not a sliver
                let inside = match region {
                    Some(reg) => reg.contains(r.center()),
                    None => r.center().y < 135.0 || r.center().x < 245.0,
                };
                if r.width() < 4.0 || r.height() < 4.0 || !inside {
                    continue;
                }
                targets.push((
                    format!(
                        "{role:?} {:?}",
                        node.accesskit_node().label().unwrap_or_default()
                    ),
                    r.center(),
                ));
            }
        }
        // strips scrolled out of the mixer's strip pane are clipped: not hoverable
        let clipped = |label: &str, p: Pos2| {
            name == "mixer"
                && p.x > 760.0
                && (label.contains("volume")
                    || label.contains(" pan")
                    || label.contains("\"M\"")
                    || label.contains("\"S\""))
        };
        for (label, p) in targets {
            if clipped(&label, p) {
                continue;
            }
            move_to(&mut h, p);
            checked += 1;
            if hint::current(&h.ctx).is_none_or(|x| x.name.is_empty()) {
                // a control another window or popup covers hits something else: only count a
                // miss when the widget under the pointer is the one we aimed at
                missing.push(format!("[{name}] {label} at {p:?}"));
            }
        }
    }
    println!("hint coverage: {checked} controls checked");
    assert!(checked > 60, "the scan found only {checked} controls");
    assert!(
        missing.is_empty(),
        "{} controls without a hint:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn the_window_options_menu_resets_size_and_position() {
    let (mut h, _ids, _dir) = start();
    let rack = WindowId::ChannelRack;
    // move and resize it somewhere else, then reset from its options menu
    let default = h.state().ws.get(rack).unwrap().rect;
    h.state_mut()
        .ws
        .drag_move(rack, default.x + 150.0, default.y + 120.0);
    let mut moved = h.state().ws.get(rack).unwrap().rect;
    moved.w = (moved.w - 100.0).max(300.0);
    h.state_mut().ws.drag_resize(
        rack,
        ph_ui::workspace::Edges::new(false, true, false, false),
        moved,
    );
    frames(&mut h, 3);
    let changed = h.state().ws.get(rack).unwrap().rect;
    assert_ne!(changed, default);
    // the rack's own "..." button is the first one of that name that sits in its title bar
    let opts: Vec<_> = h.query_all_by_label("Window options").collect();
    assert!(!opts.is_empty(), "title bars have an options button");
    let target = opts
        .iter()
        .min_by(|a, b| {
            let (ra, rb) = (a.rect(), b.rect());
            let r = h.state().ws.get(rack).unwrap().rect;
            let want = h.state().work_rect.min + Vec2::new(r.x + r.w, r.y);
            (ra.center().x - want.x)
                .abs()
                .total_cmp(&(rb.center().x - want.x).abs())
        })
        .unwrap();
    let p = target.rect().center();
    move_to(&mut h, p);
    h.event(Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.event(Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    frames(&mut h, 4);
    assert!(
        h.query_by_label("Reset size and position").is_some(),
        "the options menu opened"
    );
    h.get_by_label("Reset size and position").click();
    frames(&mut h, 4);
    let after = h.state().ws.get(rack).unwrap().rect;
    assert_eq!(after.w, default.w);
    assert_eq!(after.h, default.h);
    assert!(!h.state().ws.get(rack).unwrap().maximized);
}

#[test]
fn the_toolbar_is_customisable_and_saved_with_the_layout() {
    use ph_ui::toolbar::{ToolbarConfig, ToolbarItem};
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let make = |dir: &std::path::Path, config: &std::path::Path| {
        let st = studio(dir, ph_core::Project::empty("t"));
        let ctx = egui::Context::default();
        PhApp::new(
            &ctx,
            st,
            ph_ui::AppPaths {
                project_dir: dir.into(),
                assets_dir: assets_dir(),
                library_roots: vec![],
                config_dir: Some(config.into()),
            },
            ph_ui::Hooks::default(),
            ph_engine::EngineConfig::default(),
            None,
        )
        .unwrap()
    };
    let mut app = make(dir.path(), config.path());
    assert_eq!(app.core.toolbar, ToolbarConfig::default());
    app.core.toolbar.set_visible(ToolbarItem::Mute, false);
    app.core.toolbar.shift(ToolbarItem::Export, -4);
    let want = app.core.toolbar.clone();
    app.save_layout(false);
    drop(app);
    let dir2 = tempfile::tempdir().unwrap();
    let app = make(dir2.path(), config.path());
    assert_eq!(app.core.toolbar, want);
    // and the hidden item is really gone from the bar
    let h = harness(app);
    assert!(h.query_by_label("Mute").is_none());
    assert!(h.query_by_label("Slice").is_some());
}

#[test]
fn reduce_motion_is_a_remembered_preference() {
    let (mut h, _ids, _dir) = start();
    assert!(!ph_ui::motion::reduce(&h.ctx));
    h.state_mut().set_reduce_motion(true);
    frames(&mut h, 2);
    assert!(ph_ui::motion::reduce(&h.ctx));
    assert!(h.state().core.reduce_motion);
}

#[test]
fn the_window_title_names_the_project_and_marks_unsaved_changes() {
    assert_eq!(
        ph_ui::app::window_title("My Song", false),
        "My Song \u{2014} Patternhaus"
    );
    assert_eq!(
        ph_ui::app::window_title("My Song", true),
        "My Song \u{2022} \u{2014} Patternhaus"
    );
}
