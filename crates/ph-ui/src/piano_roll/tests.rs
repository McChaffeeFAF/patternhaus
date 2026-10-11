use super::*;
use crate::theme::Theme;
use ph_commands::{Caller, NullHost, Registry, Session};
use serde_json::json;
use std::time::Instant;

/// A project with one pattern, one sampler channel and `n` notes spread over `bars` bars.
pub fn project_with_notes(n: usize, bars: u64) -> (Project, PatternId, ChannelId) {
    let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("t"));
    let mut host = NullHost::default();
    let who = Caller::human();
    let mut run =
        |c: &str, p: serde_json::Value| s.execute(&who, Request::new(c, p), &mut host).unwrap();
    let pat = run("pattern.add", json!({})).result["pattern"]
        .as_u64()
        .unwrap() as u32;
    let ch = run(
        "channel.add",
        json!({"name": "Lead", "instrument": {"kind": "synth", "preset": "lead/saw"}}),
    )
    .result["channel"]
        .as_u64()
        .unwrap() as u32;
    let total = bars * 3840;
    let notes: Vec<_> = (0..n as u64)
        .map(|i| {
            // deterministic scatter
            let start = (i * 7919 * 15) % total / 120 * 120;
            let key = 36 + ((i * 31) % 61) as u8;
            json!({"key": key, "start": start, "length": 120 + (i % 4) * 120, "velocity": 0.3 + (i % 7) as f32 * 0.1})
        })
        .collect();
    run(
        "pattern.set_length",
        json!({"pattern": pat, "length": total}),
    );
    run(
        "note.add",
        json!({"pattern": pat, "channel": ch, "notes": notes}),
    );
    let p = s.project().as_ref().clone();
    (p, PatternId(pat), ChannelId(ch))
}

fn small() -> (PianoRollState, Project, PatternId, ChannelId) {
    let (p, pat, ch) = project_with_notes(0, 4);
    let mut s = Session::new(Arc::new(Registry::with_builtins()), p);
    let mut host = NullHost::default();
    let who = Caller::human();
    s.execute(
        &who,
        Request::new(
            "note.add",
            json!({"pattern": pat, "channel": ch, "notes": [
                {"key": 60, "start": 0, "length": 240},
                {"key": 64, "start": 480, "length": 480},
            ]}),
        ),
        &mut host,
    )
    .unwrap();
    let p = s.project().as_ref().clone();
    let mut st = PianoRollState::default();
    st.focus(pat, ch);
    st.sync(&p);
    (st, p, pat, ch)
}

/// Grid-relative centre of a note.
fn centre(st: &PianoRollState, n: &Note) -> Vec2 {
    let r = st.view.note_rect(n.key, n.start, n.length);
    Vec2::new(r.center().x, r.center().y)
}

#[test]
fn snapping() {
    assert_eq!(snap_tick(119.0, 240), 0);
    assert_eq!(snap_tick(121.0, 240), 240);
    assert_eq!(snap_floor(239.0, 240), 0);
    assert_eq!(snap_tick(123.4, 1), 123);
    assert_eq!(snap_tick(-5.0, 240), 0);
    // quantize pulls toward the grid by strength
    assert_eq!(quantize_tick(250, 240, 1.0), 240);
    assert_eq!(quantize_tick(250, 240, 0.5), 245);
    assert_eq!(quantize_tick(100, 240, 1.0), 0);
}

#[test]
fn note_names_follow_fl() {
    assert_eq!(note_name(60), "C5");
    assert_eq!(note_name(61), "C#5");
    assert_eq!(note_name(0), "C0");
    assert!(is_black(61) && !is_black(60));
}

#[test]
fn view_round_trips() {
    let v = View::default();
    for t in [0.0, 100.0, 5000.0] {
        assert!((v.tick_at(v.x_of(t)) - t).abs() < 1e-3);
    }
    let r = v.note_rect(60, 480, 240);
    assert_eq!(v.key_at(r.center().y), 60);
    let mut z = v;
    z.zoom_x(2.0, 100.0);
    assert!((z.tick_at(100.0) - v.tick_at(100.0)).abs() < 1.0);
}

#[test]
fn hit_testing_finds_body_and_resize_edge() {
    let (st, ..) = small();
    let n = st.cache.notes[1].clone();
    let r = st.view.note_rect(n.key, n.start, n.length);
    let mid = Vec2::new(r.center().x, r.center().y);
    assert_eq!(st.note_at(mid), Some((n.id, Zone::Body)));
    let edge = Vec2::new(r.right() - 1.0, r.center().y);
    assert_eq!(st.note_at(edge), Some((n.id, Zone::ResizeRight)));
    assert_eq!(st.note_at(Vec2::new(r.center().x, r.top() - 20.0)), None);
}

#[test]
fn click_empty_draws_a_note_at_the_snapped_cell_with_last_length() {
    let (mut st, _, pat, ch) = small();
    st.last_len = 480;
    // inside bar 2, key 70
    let x = st.view.x_of(3840.0 + 250.0) + 1.0;
    let y = st.view.y_of(70) + 3.0;
    let pos = Vec2::new(x, y);
    let mut out = st.pointer_press(pos, Btn::Primary, Mods::default());
    out.extend(st.pointer_release(pos, Mods::default()));
    let cmd = out
        .iter()
        .find_map(|o| match o {
            PrOut::Cmd(r) => Some(r.clone()),
            _ => None,
        })
        .expect("a command");
    assert_eq!(cmd.command, "note.add");
    assert_eq!(cmd.params["pattern"], json!(pat));
    assert_eq!(cmd.params["channel"], json!(ch));
    let n = &cmd.params["notes"][0];
    assert_eq!(n["key"], 70);
    assert_eq!(n["start"], 3840 + 240); // floor of 250 to the 240 grid
    assert_eq!(n["length"], 480);
    assert!(out.contains(&PrOut::SelectCreated));
}

#[test]
fn drag_moves_selection_by_snapped_delta() {
    let (mut st, _, pat, ch) = small();
    let n = st.cache.notes[0].clone();
    let c = centre(&st, &n);
    st.pointer_press(c, Btn::Primary, Mods::default());
    assert!(st.selection.contains(&n.id));
    // drag 2 steps right (+500 ticks raw -> snaps to 480) and 3 keys up
    let to = c + Vec2::new(500.0 * st.view.px_per_tick as f32, -3.0 * st.view.key_h);
    st.pointer_move(to, Mods::default());
    assert_eq!(st.display_geom(&n), (63, 480, 240));
    let out = st.pointer_release(to, Mods::default());
    let cmd = out
        .into_iter()
        .find_map(|o| if let PrOut::Cmd(r) = o { Some(r) } else { None })
        .unwrap();
    assert_eq!(cmd.command, "note.move");
    assert_eq!(cmd.params["delta_ticks"], 480);
    assert_eq!(cmd.params["delta_keys"], 3);
    assert_eq!(cmd.params["pattern"], json!(pat));
    assert_eq!(cmd.params["channel"], json!(ch));
}

#[test]
fn a_click_on_a_note_only_selects_and_previews() {
    let (mut st, ..) = small();
    let n = st.cache.notes[0].clone();
    let c = centre(&st, &n);
    let mut out = st.pointer_press(c, Btn::Primary, Mods::default());
    out.extend(st.pointer_release(c, Mods::default()));
    assert!(out.iter().all(|o| matches!(o, PrOut::Preview { .. })));
    assert_eq!(st.selection.len(), 1);
}

#[test]
fn right_drag_erases_every_note_it_passes() {
    let (mut st, ..) = small();
    let (a, b) = (st.cache.notes[0].clone(), st.cache.notes[1].clone());
    st.pointer_press(centre(&st, &a), Btn::Secondary, Mods::default());
    st.pointer_move(centre(&st, &b), Mods::default());
    let out = st.pointer_release(centre(&st, &b), Mods::default());
    let cmd = out
        .into_iter()
        .find_map(|o| if let PrOut::Cmd(r) = o { Some(r) } else { None })
        .unwrap();
    assert_eq!(cmd.command, "note.remove");
    assert_eq!(cmd.params["ids"].as_array().unwrap().len(), 2);
}

#[test]
fn resize_from_the_right_edge_and_remember_the_length() {
    let (mut st, ..) = small();
    let n = st.cache.notes[1].clone(); // 480 long at 480
    let r = st.view.note_rect(n.key, n.start, n.length);
    let edge = Vec2::new(r.right() - 1.0, r.center().y);
    st.pointer_press(edge, Btn::Primary, Mods::default());
    let to = edge + Vec2::new(240.0 * st.view.px_per_tick as f32, 0.0);
    st.pointer_move(to, Mods::default());
    let out = st.pointer_release(to, Mods::default());
    let cmd = out
        .into_iter()
        .find_map(|o| if let PrOut::Cmd(r) = o { Some(r) } else { None })
        .unwrap();
    assert_eq!(cmd.command, "note.resize");
    assert_eq!(cmd.params["delta"], 240);
    assert_eq!(st.last_len, 720);
}

#[test]
fn box_select_picks_notes_inside() {
    let (mut st, ..) = small();
    st.tool = Tool::Select;
    let from = Vec2::new(-5.0, 0.0);
    let to = Vec2::new(st.view.x_of(300.0), 2000.0);
    st.pointer_press(from, Btn::Primary, Mods::default());
    st.pointer_move(to, Mods::default());
    st.pointer_release(to, Mods::default());
    assert_eq!(st.selection.len(), 1);
    st.action(PrAction::SelectAll);
    assert_eq!(st.selection.len(), 2);
}

#[test]
fn keyboard_actions_map_to_commands() {
    let (mut st, ..) = small();
    st.action(PrAction::SelectAll);
    let cmd = |v: Vec<PrOut>| {
        v.into_iter()
            .find_map(|o| if let PrOut::Cmd(r) = o { Some(r) } else { None })
            .unwrap()
    };
    let dup = cmd(st.action(PrAction::Duplicate));
    assert_eq!(dup.command, "note.copy");
    assert_eq!(dup.params["offset"], 960); // selection spans 0..960
    assert_eq!(cmd(st.action(PrAction::Quantize)).command, "note.quantize");
    assert_eq!(cmd(st.action(PrAction::OctaveUp)).params["semitones"], 12);
    assert_eq!(
        cmd(st.action(PrAction::TransposeDown)).params["semitones"],
        -1
    );
    assert_eq!(
        cmd(st.action(PrAction::DeleteSelected)).command,
        "note.remove"
    );
    assert!(st.selection.is_empty());
}

#[test]
fn copy_and_paste_round_trips_through_note_add() {
    let (mut st, ..) = small();
    st.action(PrAction::SelectAll);
    st.action(PrAction::Copy);
    assert_eq!(st.clipboard.len(), 2);
    let out = st.paste(3840);
    let PrOut::Cmd(r) = &out[0] else { panic!() };
    assert_eq!(r.command, "note.add");
    assert_eq!(r.params["notes"][1]["start"], 3840 + 480);
}

#[test]
fn copy_and_paste_keeps_muted_notes_muted() {
    let (mut st, ..) = small();
    st.action(PrAction::SelectAll);
    st.action(PrAction::Copy);
    // mute the first copied note as if it had been muted in the pattern
    st.clipboard[0].muted = true;
    let out = st.paste(0);
    let PrOut::Cmd(r) = &out[0] else { panic!() };
    assert_eq!(r.params["notes"][0]["muted"], true);
    assert_eq!(r.params["notes"][1]["muted"], false);
}

#[test]
fn velocity_lane_drag_sets_velocity() {
    let (mut st, ..) = small();
    let n = st.cache.notes[1].clone();
    let x = st.view.x_of(n.start as f64) + 1.0;
    assert!(st.velocity_press(Vec2::new(x, 32.0), 64.0));
    st.pointer_move(Vec2::new(x, 16.0), Mods::default());
    let out = st.pointer_release(Vec2::new(x, 16.0), Mods::default());
    let PrOut::Cmd(r) = &out[0] else { panic!() };
    assert_eq!(r.command, "note.set_velocity");
    assert!((r.params["velocity"].as_f64().unwrap() - 0.75).abs() < 1e-6);
}

#[test]
fn cache_refreshes_only_when_the_pattern_changes() {
    let (mut st, p, ..) = small();
    let before = st.cache.notes.as_ptr();
    st.sync(&p);
    assert_eq!(before, st.cache.notes.as_ptr());
}

// ---- the spike: 10 000 notes -----------------------------------------------------------

fn run_frame(
    ctx: &egui::Context,
    st: &mut PianoRollState,
    p: &Project,
    theme: &Theme,
    i: usize,
) -> f64 {
    let flashes = [];
    let agent = BTreeSet::new();
    let t = Instant::now();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 900.0))),
        time: Some(i as f64 / 60.0),
        ..Default::default()
    };
    let full = ctx.run_ui(input, |ui| {
        let cx = DrawCtx {
            project: p,
            theme,
            playhead: Some(1000),
            flashes: &flashes,
            now: i as f64 / 60.0,
            agent_selected: &agent,
        };
        let _ = show(ui, st, &cx);
    });
    let mut full = full;
    full.textures_delta.clear();
    let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
    std::hint::black_box(prims.len());
    t.elapsed().as_secs_f64() * 1000.0
}

/// DESIGN/brief spike: 10 000 notes, scroll + zoom + drag, CPU side (layout, paint and
/// tessellation). Release builds are the real number; this also guards debug builds loosely.
#[test]
fn spike_ten_thousand_notes_frame_cost() {
    let (p, pat, ch) = project_with_notes(10_000, 64);
    assert_eq!(p.pattern(pat).unwrap().note_count(), 10_000);
    let theme = Theme::default_theme();
    let ctx = egui::Context::default();
    theme.apply(&ctx);
    let mut st = PianoRollState::default();
    st.focus(pat, ch);
    st.sync(&p);
    // Several zoom levels: all 64 bars in view (worst case for culling), and zoomed in.
    let mut samples: Vec<(&str, Vec<f64>)> = Vec::new();
    for (label, ppt) in [
        ("all 64 bars visible", 0.004),
        ("8 bars visible", 0.04),
        ("1 bar visible", 0.35),
    ] {
        st.view.px_per_tick = ppt;
        st.view.x0 = 0.0;
        st.view.y0 = 100.0;
        st.view.key_h = 12.0;
        // warm-up (fonts, atlas)
        for i in 0..5 {
            run_frame(&ctx, &mut st, &p, &theme, i);
        }
        // start a drag of the whole selection so the preview path is exercised
        st.action(PrAction::SelectAll);
        let n = st.cache.notes[st.cache.notes.len() / 2].clone();
        let c = centre(&st, &n);
        let mut frames = Vec::new();
        for i in 0..120 {
            // scroll + zoom each frame
            st.view.x0 = (i as f64 * 600.0) % 200_000.0;
            st.view.y0 = 100.0 - (i % 20) as f64;
            if i == 10 {
                st.pointer_press(c, Btn::Primary, Mods::default());
            }
            if i > 10 {
                st.pointer_move(
                    c + Vec2::new((i % 40) as f32, (i % 5) as f32),
                    Mods::default(),
                );
            }
            frames.push(run_frame(&ctx, &mut st, &p, &theme, 5 + i));
        }
        st.pointer_release(c, Mods::default());
        samples.push((label, frames));
    }
    let mut worst_avg = 0.0f64;
    for (label, mut f) in samples {
        f.sort_by(|a, b| a.total_cmp(b));
        let avg = f.iter().sum::<f64>() / f.len() as f64;
        let p95 = f[(f.len() as f64 * 0.95) as usize];
        println!(
            "SPIKE piano roll 10k notes, {label}: avg {avg:.2} ms, p95 {p95:.2} ms, max {:.2} ms",
            f.last().unwrap()
        );
        worst_avg = worst_avg.max(avg);
    }
    // Debug builds are 5-10x slower than release; the real budget (8 ms) is asserted in release.
    let budget = if cfg!(debug_assertions) { 60.0 } else { 8.0 };
    assert!(
        worst_avg < budget,
        "avg {worst_avg:.2} ms exceeds {budget} ms"
    );
}

#[test]
fn a_lasso_selects_by_shape_not_by_bounding_box() {
    let (mut st, ..) = small();
    let notes: Vec<Note> = st.cache.notes.clone();
    let (a, b) = (centre(&st, &notes[0]), centre(&st, &notes[1]));
    // a triangle around `a` only; `b` is inside its bounding box but outside the triangle
    let tri = [
        Vec2::new(a.x - 30.0, b.y - 40.0),
        Vec2::new(a.x - 30.0, a.y + 30.0),
        Vec2::new(b.x + 30.0, a.y + 30.0),
    ];
    assert!(
        b.x < tri[2].x && (tri[0].y..tri[1].y).contains(&b.y),
        "setup: b is in the box"
    );
    assert_eq!(st.notes_in_lasso(&tri), vec![notes[0].id]);
    // too few points select nothing, and a loop around both selects both
    assert!(st.notes_in_lasso(&tri[..2]).is_empty());
    let (top, bot) = (a.y.min(b.y) - 40.0, a.y.max(b.y) + 40.0);
    let both = [
        Vec2::new(a.x - 30.0, top),
        Vec2::new(b.x + 30.0, top),
        Vec2::new(b.x + 30.0, bot),
        Vec2::new(a.x - 30.0, bot),
    ];
    assert_eq!(st.notes_in_lasso(&both).len(), 2);
    // Alt+drag on empty space is a lasso, plain drag in Draw would have drawn a note
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };
    let far = Vec2::new(a.x, a.y - 200.0);
    st.pointer_press(far, Btn::Primary, alt);
    assert!(st.is_dragging());
    for p in both {
        st.pointer_move(p, alt);
    }
    let out = st.pointer_release(both[0], alt);
    assert!(out.is_empty(), "no command: selection only");
}

#[test]
fn slice_and_mute_tools_emit_one_batch_and_one_update() {
    let (mut st, ..) = small();
    let n = st.cache.notes[1].clone(); // key 64, start 480, length 480
    st.tool = Tool::Slice;
    let at = st.view.x_of(720.0);
    let c = centre(&st, &n);
    let out = st.pointer_press(Vec2::new(at, c.y), Btn::Primary, Mods::default());
    let PrOut::Batch(reqs) = &out[0] else {
        panic!("{out:?}")
    };
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].command, "note.update");
    assert_eq!(reqs[0].params["updates"][0]["length"], 240);
    assert_eq!(reqs[1].command, "note.add");
    assert_eq!(reqs[1].params["notes"][0]["start"], 720);
    assert_eq!(reqs[1].params["notes"][0]["length"], 240);
    // at the very start of a note: nothing to cut
    let at = st.view.x_of(480.0);
    assert!(
        st.pointer_press(Vec2::new(at + 0.5, c.y), Btn::Primary, Mods::default())
            .is_empty()
    );
    // mute: press + release emits one note.update
    st.tool = Tool::Mute;
    st.pointer_press(c, Btn::Primary, Mods::default());
    let out = st.pointer_release(c, Mods::default());
    assert_eq!(out.len(), 1);
    let PrOut::Cmd(r) = &out[0] else { panic!() };
    assert_eq!(r.command, "note.update");
    assert_eq!(r.params["updates"][0]["muted"], true);
}
