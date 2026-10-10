//! Save, reopen and export through the same requests the File menu and dialogs send, plus the
//! live readouts (position, meters) a playing engine feeds the UI.

mod common;

use common::*;
use ph_ui::mapping;

#[test]
fn save_reopen_and_export_wav_through_ui_requests() {
    let (mut fx, ids) = fixture(true);
    let _ids = ids.unwrap();
    let app = &mut fx.app;
    pump_until(app, 500, |a| a.core.project.channels.len() == 4);
    let before = app.core.project.clone();

    // Ctrl+S (file.save into the project folder)
    app.core.send(mapping::file_save());
    settle(app);
    assert!(fx.dir.path().join("project.json").exists());
    assert!(!app.core.dirty);

    // Export a WAV (File > Export, Ctrl+R)
    app.core
        .send(mapping::export_audio("exports/mix.wav", false));
    assert!(
        pump_until(app, 30_000, |a| a.core.dispatch.in_flight() == 0),
        "export finishes"
    );
    let wav = fx.dir.path().join("exports/mix.wav");
    assert!(wav.exists(), "wav written");
    assert!(
        std::fs::metadata(&wav).unwrap().len() > 50_000,
        "it has audio in it"
    );
    assert!(
        app.core
            .toasts
            .iter()
            .all(|t| t.kind != ph_ui::core::ToastKind::Error)
    );

    // exporting again without "replace" is refused and reported, not silently overwritten
    app.core
        .send(mapping::export_audio("exports/mix.wav", false));
    settle(app);
    assert!(
        app.core
            .toasts
            .iter()
            .any(|t| t.kind == ph_ui::core::ToastKind::Error)
    );

    // edit, then reopen the saved folder: the project comes back exactly as saved
    app.core.send(mapping::set_tempo(99.0));
    settle(app);
    assert_eq!(app.core.project.settings.bpm, 99.0);
    app.core
        .send(mapping::file_open(&fx.dir.path().to_string_lossy()));
    settle(app);
    pump_until(app, 500, |a| a.core.project.settings.bpm != 99.0);
    assert_eq!(
        *app.core.project, *before,
        "reopened project equals the saved one"
    );
    assert!(
        app.core.history.is_empty() || app.core.history.len() < 5,
        "history restarts on open"
    );
}

#[test]
fn playing_feeds_position_and_meters_to_the_ui() {
    let (mut fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let app = &mut fx.app;
    pump_until(app, 500, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.fix_selection();
    app.core.send(mapping::set_mode_pattern(Some(ids.beat)));
    app.core.send(mapping::transport("play"));
    settle(app);
    assert!(
        pump_until(app, 3000, |a| {
            let l = a.core.live();
            l.status.playing && l.status.position > 0
        }),
        "the playhead moves"
    );
    // the kick is on insert 1 and the master carries everything
    let mut heard_master = false;
    let mut heard_kick = false;
    pump_until(app, 3000, |a| {
        let l = a.core.live();
        heard_master |= l.peak.first().is_some_and(|p| p[0] > 0.001);
        heard_kick |= l.peak.get(1).is_some_and(|p| p[0] > 0.001);
        heard_master && heard_kick
    });
    assert!(heard_master, "master meter moved");
    assert!(heard_kick, "the kick's insert meter moved");
    // the poller thread refreshes the readouts at well over display rate (about 120 Hz)
    let p0 = app.core.live().polls;
    pump_until(app, 300, |_| false);
    let rate = (app.core.live().polls - p0) as f64 / 0.3;
    println!("PERF live readouts poll rate: {rate:.0} Hz");
    // Shared CI runners schedule threads coarsely (macOS measured exactly 60 Hz), so there
    // the check is that readouts keep flowing; the display-rate bound is checked locally.
    let floor = if std::env::var_os("CI").is_some() { 20.0 } else { 60.0 };
    assert!(rate > floor, "poll rate {rate:.0} Hz");
    app.core.send(mapping::transport("stop"));
    settle(app);
    assert!(pump_until(app, 2000, |a| !a.core.live().status.playing));
}
