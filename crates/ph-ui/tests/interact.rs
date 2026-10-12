//! Mouse gestures through real egui pointer events, end to end: gesture -> command -> session
//! -> snapshot -> redraw. Positions come from the geometry the panels record while drawing.

mod common;

use common::*;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use egui_kittest::Harness;
use ph_core::{ChannelId, STEP};
use ph_ui::PhApp;
use ph_ui::core::PanelId;
use ph_ui::steps;
use std::time::Duration;

type H = Harness<'static, PhApp>;

fn start() -> (H, Ids, tempfile::TempDir) {
    let (fx, ids) = fixture(true);
    let ids = ids.unwrap();
    let mut app = fx.app;
    pump_until(&mut app, 500, |a| a.core.project.channels.len() == 4);
    app.core.sel.pattern = Some(ids.beat);
    app.core.sel.channel = Some(ids.kick);
    app.core.fix_selection();
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .build_ui_state(|ui, app: &mut PhApp| app.draw(ui), app);
    for _ in 0..4 {
        h.step();
    }
    (h, ids, fx.dir)
}

fn frames(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn button(h: &mut H, pos: Pos2, b: PointerButton, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: b,
        pressed,
        modifiers: Modifiers::NONE,
    });
    h.step();
}

fn click(h: &mut H, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    button(h, pos, PointerButton::Primary, true);
    button(h, pos, PointerButton::Primary, false);
    frames(h, 6);
}

fn drag(h: &mut H, from: Pos2, to: Pos2, b: PointerButton) {
    h.event(Event::PointerMoved(from));
    h.step();
    button(h, from, b, true);
    let n = 6;
    for i in 1..=n {
        let p = from + (to - from) * (i as f32 / n as f32);
        h.event(Event::PointerMoved(p));
        h.step();
    }
    button(h, to, b, false);
    frames(h, 6);
}

fn step_centre(h: &H, ch: ChannelId, step: u32) -> Pos2 {
    let (_, rect, lay) = h
        .state()
        .core
        .rack
        .geometry
        .iter()
        .find(|(c, ..)| *c == ch)
        .expect("row was drawn");
    Pos2::new(lay.x_of(step) + lay.w / 2.0 - 1.0, rect.center().y)
}

fn row(h: &H, ids: &Ids, ch: ChannelId) -> Vec<bool> {
    steps::row(&h.state().core.project, ids.beat, ch, 16)
        .iter()
        .map(|s| s.is_on())
        .collect()
}

#[test]
fn clicking_and_painting_steps() {
    let (mut h, ids, _dir) = start();
    // front: the channel rack is in the default layout
    assert!(!h.state().core.rack.geometry.is_empty());

    // a click lights an unlit step: step.toggle
    assert!(!row(&h, &ids, ids.snare)[1]);
    let p = step_centre(&h, ids.snare, 1);
    click(&mut h, p);
    assert!(
        row(&h, &ids, ids.snare)[1],
        "step 2 of the snare row is lit"
    );

    // clicking it again clears it
    click(&mut h, p);
    assert!(!row(&h, &ids, ids.snare)[1]);

    // dragging across a row paints every step it touches in one command
    let before = h.state().core.dispatch.sent;
    let (a, b) = (step_centre(&h, ids.hat, 8), step_centre(&h, ids.hat, 11));
    drag(&mut h, a, b, PointerButton::Primary);
    let hat = row(&h, &ids, ids.hat);
    assert!(hat[8] && hat[9] && hat[10] && hat[11], "{hat:?}");
    assert_eq!(
        h.state().core.dispatch.sent,
        before + 1,
        "one command for the whole stroke"
    );

    // right-drag erases
    drag(&mut h, b, a, PointerButton::Secondary);
    let hat = row(&h, &ids, ids.hat);
    assert!(!hat[8] && !hat[9] && !hat[11], "{hat:?}");

    // one undo step reverts the last stroke (the erase), not more
    let kick_before = row(&h, &ids, ids.kick);
    h.state_mut().core.send(ph_ui::mapping::undo_mine());
    settle(h.state_mut());
    frames(&mut h, 3);
    assert!(
        row(&h, &ids, ids.hat)[8],
        "undo brought the painted steps back"
    );
    assert_eq!(row(&h, &ids, ids.kick), kick_before);
}

#[test]
fn drawing_moving_and_deleting_notes_in_the_piano_roll() {
    let (mut h, ids, _dir) = start();
    h.state_mut().open_panel(PanelId::PianoRoll);
    h.state_mut().core.sel.channel = Some(ids.bass);
    h.state_mut().core.sel.pattern = Some(ids.bassline);
    h.state_mut().core.fix_selection();
    frames(&mut h, 4);
    let notes = |h: &H| {
        h.state().core.project.pattern(ids.bassline).unwrap().notes[&ids.bass]
            .values()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(notes(&h).len(), 4);

    let (grid, view) = {
        let r = &h.state().core.roll;
        (r.last_grid, r.view)
    };
    assert!(grid.width() > 100.0, "the roll was drawn");
    // click in empty space: bar 1, beat 3.5 (tick 2400 -> snaps to the 240 grid), key 50
    let at = |tick: f64, key: u8| {
        Pos2::new(
            grid.left() + view.x_of(tick) + 1.0,
            grid.top() + view.y_of(key) + view.key_h / 2.0,
        )
    };
    // a key row near the middle of the view with nothing in it
    let mid = view.key_at(grid.height() / 2.0);
    let free_key = (0..10)
        .flat_map(|d| [mid + d, mid - d])
        .map(|k| k as u8)
        .find(|k| !notes(&h).iter().any(|n| n.key == *k))
        .unwrap();
    let target = at(2410.0, free_key);
    click(&mut h, target);
    let after = notes(&h);
    assert_eq!(after.len(), 5, "a click draws a note");
    let drawn = after
        .iter()
        .find(|n| n.key == free_key)
        .expect("drawn at the clicked key");
    assert_eq!(drawn.start, 2400);
    assert_eq!(drawn.length, STEP);
    assert!(
        h.state().core.roll.selection.contains(&drawn.id),
        "the new note is selected"
    );

    // drag it two steps right and one key up
    let (id, start, key) = (drawn.id, drawn.start, drawn.key);
    let from = at(start as f64 + 60.0, key);
    let to = from + Vec2::new(480.0 * view.px_per_tick as f32, -view.key_h);
    drag(&mut h, from, to, PointerButton::Primary);
    let moved = notes(&h).into_iter().find(|n| n.id == id).unwrap();
    assert_eq!((moved.start, moved.key), (start + 480, key + 1));

    // right-click deletes it
    let at_note = at(moved.start as f64 + 60.0, moved.key);
    h.event(Event::PointerMoved(at_note));
    h.step();
    button(&mut h, at_note, PointerButton::Secondary, true);
    button(&mut h, at_note, PointerButton::Secondary, false);
    frames(&mut h, 6);
    assert_eq!(notes(&h).len(), 4);
}

#[test]
fn placing_and_moving_clips_in_the_playlist() {
    let (mut h, ids, _dir) = start();
    h.state_mut().open_panel(PanelId::Playlist);
    frames(&mut h, 4);
    let pl = &h.state().core.playlist;
    let grid = pl.last_grid;
    let at = |tick: f64, track: i32| {
        Pos2::new(
            grid.left() + pl.x_of(tick) + 3.0,
            grid.top() + pl.y_of(track) + pl.track_h / 2.0,
        )
    };
    let clips_before = h.state().core.project.clips.len();
    assert_eq!(clips_before, 14);
    // the selected pattern (the beat) goes on track 6 at bar 2 (snaps to the bar)
    let p = at(3840.0 + 500.0, 5);
    click(&mut h, p);
    let project = h.state().core.project.clone();
    assert_eq!(project.clips.len(), clips_before + 1);
    let new = project
        .clips
        .values()
        .find(|c| c.track == 5)
        .expect("clip on track 6");
    assert_eq!((new.start, new.pattern()), (3840, Some(ids.beat)));
}

// ---- piano roll: lasso, slice and mute --------------------------------------------------

fn mod_frame(h: &mut H, _m: Modifiers) {
    h.step();
}

fn mouse(h: &mut H, pos: Pos2, b: PointerButton, pressed: bool, m: Modifiers) {
    h.event(Event::PointerButton {
        pos,
        button: b,
        pressed,
        modifiers: m,
    });
    mod_frame(h, m);
}

/// Press at the first point, move through the rest, release at the last, with `m` held.
fn drag_path(h: &mut H, path: &[Pos2], m: Modifiers) {
    h.event(Event::ModifiersChanged(m));
    h.event(Event::PointerMoved(path[0]));
    mod_frame(h, m);
    mouse(h, path[0], PointerButton::Primary, true, m);
    for p in &path[1..] {
        h.event(Event::PointerMoved(*p));
        mod_frame(h, m);
    }
    mouse(h, *path.last().unwrap(), PointerButton::Primary, false, m);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    frames(h, 6);
}

fn roll(h: &mut H, ids: &Ids) -> impl Fn(f64, f64) -> Pos2 + use<> {
    h.state_mut().open_panel(PanelId::PianoRoll);
    h.state_mut().core.sel.channel = Some(ids.bass);
    h.state_mut().core.sel.pattern = Some(ids.bassline);
    h.state_mut().core.fix_selection();
    frames(h, 4);
    let (grid, view) = {
        let r = &h.state().core.roll;
        (r.last_grid, r.view)
    };
    assert!(grid.width() > 100.0, "the roll was drawn");
    // a point at a tick and a (fractional) key row
    move |tick: f64, key: f64| {
        Pos2::new(
            grid.left() + view.x_of(tick) + 1.0,
            grid.top() + ((view.y0 - (key + 1.0)) as f32) * view.key_h + view.key_h / 2.0,
        )
    }
}

fn bass_notes(h: &H, ids: &Ids) -> Vec<ph_core::Note> {
    h.state().core.project.pattern(ids.bassline).unwrap().notes[&ids.bass]
        .values()
        .cloned()
        .collect()
}

#[test]
fn alt_drag_lassos_notes_and_ctrl_drag_still_boxes() {
    let (mut h, ids, _dir) = start();
    let at = roll(&mut h, &ids);
    let sorted = |h: &H| {
        let mut v = bass_notes(h, &ids);
        v.sort_by_key(|n| n.start);
        v
    };
    let notes = sorted(&h);
    assert_eq!(notes.len(), 4);
    assert!(h.state().core.roll.selection.is_empty());
    // a loop (drawn by hand, starting in empty space) around the first two notes: starts above
    // them, comes down around, and ends back near where it began
    let k = 36.0;
    let lasso = [
        at(60.0, k + 2.0),
        at(1500.0, k + 2.0),
        at(1500.0, k - 1.5),
        at(60.0, k - 1.5),
        at(60.0, k + 1.8),
    ];
    drag_path(&mut h, &lasso, Modifiers::ALT);
    let sel: Vec<_> = h.state().core.roll.selection.iter().copied().collect();
    assert_eq!(
        sel,
        vec![notes[0].id, notes[1].id],
        "the lasso took two notes"
    );
    assert_eq!(
        bass_notes(&h, &ids).len(),
        4,
        "alt+drag in empty space draws nothing"
    );
    // ctrl+drag is still a box, which here catches the later note too
    let from = at(60.0, k + 5.0);
    let to = at(3300.0, k - 4.0);
    drag_path(&mut h, &[from, to], Modifiers::COMMAND);
    assert!(
        h.state().core.roll.selection.len() >= 3,
        "box select still works"
    );
}

#[test]
fn the_slice_tool_splits_a_note_at_the_snapped_click_as_one_undo_step() {
    let (mut h, ids, _dir) = start();
    let at = roll(&mut h, &ids);
    h.state_mut().core.roll.tool = ph_ui::piano_roll::Tool::Slice;
    let grid = h.state().core.roll.grid();
    let before = bass_notes(&h, &ids);
    let first = before.iter().min_by_key(|n| n.start).unwrap().clone();
    assert_eq!((first.start, first.length), (0, 720));
    // click 500 ticks into the note: the cut snaps to the grid
    click(&mut h, at(500.0, first.key as f64));
    let mut after = bass_notes(&h, &ids);
    after.sort_by_key(|n| (n.start, n.key));
    assert_eq!(after.len(), 5, "one note became two");
    let cut = snap(500, grid);
    let head = after.iter().find(|n| n.id == first.id).unwrap();
    assert_eq!((head.start, head.length), (0, cut));
    let tail = after
        .iter()
        .find(|n| n.start == cut && n.key == first.key)
        .expect("a note starts at the cut");
    assert_eq!(tail.start + tail.length, 720, "the tail keeps the old end");
    assert_eq!(tail.velocity, first.velocity);
    // clicking exactly where a note starts (or past its end) changes nothing
    click(&mut h, at(0.0, first.key as f64 + 0.0));
    assert_eq!(bass_notes(&h, &ids).len(), 5);
    // a single undo step puts the original note back
    h.state_mut().core.send(ph_ui::mapping::undo_mine());
    frames(&mut h, 8);
    let undone = bass_notes(&h, &ids);
    assert_eq!(undone.len(), 4, "both halves of the split undone together");
    let n = undone.iter().find(|n| n.id == first.id).unwrap();
    assert_eq!((n.start, n.length), (0, 720));
}

fn snap(t: u64, grid: u64) -> u64 {
    ((t as f64 / grid as f64).round() as u64) * grid
}

#[test]
fn the_mute_tool_toggles_notes_and_a_drag_sets_them_all_alike() {
    let (mut h, ids, _dir) = start();
    let at = roll(&mut h, &ids);
    h.state_mut().core.roll.tool = ph_ui::piano_roll::Tool::Mute;
    let notes = |h: &H| {
        let mut v = bass_notes(h, &ids);
        v.sort_by_key(|n| n.start);
        v
    };
    let n0 = notes(&h)[0].clone();
    assert!(!n0.muted);
    click(&mut h, at(300.0, n0.key as f64));
    assert!(notes(&h)[0].muted, "a click mutes");
    assert_eq!(bass_notes(&h, &ids).len(), 4, "the note is still there");
    click(&mut h, at(300.0, n0.key as f64));
    assert!(!notes(&h)[0].muted, "and a second click unmutes");
    // dragging across the first two notes (same key row) mutes both
    drag(
        &mut h,
        at(300.0, n0.key as f64),
        at(1500.0, n0.key as f64),
        PointerButton::Primary,
    );
    let v = notes(&h);
    assert!(v[0].muted && v[1].muted && !v[2].muted, "{v:?}");
    // one undo step for the whole stroke
    h.state_mut().core.send(ph_ui::mapping::undo_mine());
    frames(&mut h, 8);
    assert!(notes(&h).iter().all(|n| !n.muted));
}

#[test]
fn the_toolbar_has_slice_and_mute_tools() {
    use egui_kittest::kittest::Queryable;
    let (mut h, ids, _dir) = start();
    let _ = roll(&mut h, &ids);
    h.get_by_label("Slice").click();
    frames(&mut h, 3);
    assert_eq!(h.state().core.roll.tool, ph_ui::piano_roll::Tool::Slice);
    h.get_by_label("Mute").click();
    frames(&mut h, 3);
    assert_eq!(h.state().core.roll.tool, ph_ui::piano_roll::Tool::Mute);
}
