//! The transport row of the top bar: PAT/SONG, play and stop, loop and metronome, the tempo
//! LCD, the large bar-beat-tick LCD, the scope and the pattern selector. The health readouts
//! (DSP, RAM) and the agent badge sit on the menu row (see [`health`]).

use crate::core::{AppCore, Dialog};
use crate::hint::HintExt;
use crate::icons::Icon;
use crate::mapping;
use crate::paint;
use crate::theme::{TRACK_CAPTION, Theme, caps_sized};
use crate::widgets::{self, btn, icon_button, icon_button_coloured, toggle};
use egui::{
    Align2, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use ph_core::{PPQ, Tick};
use ph_engine::PlayMode;

/// `bar:beat:tick`, 1-based bars and beats.
pub fn format_position(pos: Tick, ticks_per_bar: Tick) -> String {
    let bar = pos / ticks_per_bar.max(1);
    let within = pos % ticks_per_bar.max(1);
    format!("{:03}:{}:{:03}", bar + 1, within / PPQ + 1, within % PPQ)
}

pub fn format_time(pos: Tick, bpm: f64) -> String {
    let secs = ph_core::ticks_to_seconds(pos, bpm);
    format!("{:02}:{:04.1}", (secs / 60.0) as u32, secs % 60.0)
}

/// A caption in the corner of an LCD: tiny, uppercase, wide tracking.
fn caption(p: &egui::Painter, pos: Pos2, text: &str, theme: &Theme) {
    let g = p.layout_no_wrap(
        text.to_uppercase(),
        FontId::new(theme.metrics.font_caption - 1.0, crate::fonts::cond()),
        theme.colors.faint,
    );
    // egui has no tracking on raw galleys here, so a space-separated caption stands in
    p.galley(pos, g, theme.colors.faint);
}

pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let live = core.live();
    let project = core.project.clone();
    let c = &theme.colors;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // PAT / SONG
        let pattern_mode = matches!(live.status.mode, PlayMode::Pattern(_));
        if toggle(ui, pattern_mode, "PAT", c.accent, &theme)
            .on_hover_text("Pattern mode loops the selected pattern (L)")
            .hint("Pattern mode", "", "loops the selected pattern (L)")
            .clicked()
        {
            core.send(mapping::set_mode_pattern(core.sel.pattern));
        }
        if toggle(ui, !pattern_mode, "SONG", c.accent, &theme)
            .on_hover_text("Song mode plays the playlist (L)")
            .hint("Song mode", "", "plays the playlist (L)")
            .clicked()
        {
            core.send(mapping::set_mode_song());
        }
        ui.add_space(4.0);

        // transport
        let playing = live.status.playing;
        if icon_button_coloured(
            ui,
            &theme,
            Icon::Play,
            playing,
            true,
            "Play",
            "start or pause (Space)",
            c.ok,
        )
        .clicked()
        {
            core.send(mapping::transport("toggle"));
        }
        if icon_button(
            ui,
            &theme,
            Icon::Stop,
            false,
            "Stop",
            "stop and return to the start",
        )
        .clicked()
        {
            core.send(mapping::transport("stop"));
        }
        let looping = live.status.loop_range.is_some();
        if icon_button(
            ui,
            &theme,
            Icon::Loop,
            looping,
            "Loop",
            "loop the song, or the region shift-dragged on the playlist ruler",
        )
        .clicked()
        {
            let end = project
                .song_length()
                .max(project.settings.time_signature.ticks_per_bar() * 4);
            core.send(mapping::set_loop(if looping {
                None
            } else {
                Some((0, end))
            }));
        }
        if icon_button_coloured(
            ui,
            &theme,
            Icon::Metronome,
            live.status.metronome,
            true,
            "Metronome",
            "click track while playing",
            c.warn,
        )
        .clicked()
        {
            core.send(mapping::set_metronome(!live.status.metronome));
        }
        ui.add_space(6.0);

        tempo_lcd(ui, core, &theme, project.settings.bpm);

        // bar : beat : tick, large
        let bpm = project.settings.bpm;
        let bar = project.settings.time_signature.ticks_per_bar();
        position_lcd(ui, &theme, live.status.position, bar, bpm);

        scope(ui, &theme, &live.scope);
        ui.add_space(8.0);

        // pattern selector
        ui.vertical(|ui| {
            ui.add_space(1.0);
            ui.label(caps_sized("Pattern", theme.metrics.font_caption, TRACK_CAPTION).color(c.dim));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let names: Vec<_> = project
                    .patterns_in_order()
                    .map(|p| (p.id, p.name.clone()))
                    .collect();
                let cur = core
                    .sel
                    .pattern
                    .and_then(|id| names.iter().find(|(i, _)| *i == id))
                    .map_or("(no pattern)".to_string(), |(_, n)| n.clone());
                egui::ComboBox::from_id_salt("pattern_select")
                    .selected_text(&cur)
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for (id, name) in &names {
                            if ui
                                .selectable_label(core.sel.pattern == Some(*id), name)
                                .clicked()
                            {
                                core.select_pattern(*id);
                            }
                        }
                    })
                    .response
                    .hint_menu("Pattern", &cur, "click to choose the pattern to edit");
                if icon_button(ui, &theme, Icon::Plus, false, "New pattern", "").clicked() {
                    core.send(mapping::pattern_add());
                }
                if icon_button(ui, &theme, Icon::Clone, false, "Clone pattern", "Ctrl+L").clicked()
                    && let Some(p) = core.sel.pattern
                {
                    core.send(mapping::pattern_clone(p));
                }
                if icon_button(ui, &theme, Icon::Rename, false, "Rename pattern", "").clicked()
                    && let Some(p) = core.sel.pattern
                {
                    core.open_rename(crate::core::RenameTarget::Pattern(p));
                }
            });
        });

        // typing keyboard
        ui.add_space(6.0);
        let on = core.typing.enabled;
        if icon_button(
            ui,
            &theme,
            Icon::Keys,
            on,
            "Typing keyboard",
            "plays the selected channel (Ctrl+T); - and = shift the octave",
        )
        .clicked()
        {
            core.typing.enabled = !on;
        }
        ui.label(
            RichText::new(format!("oct {}", core.typing.octave))
                .font(FontId::new(11.0, crate::fonts::mono()))
                .color(if on { c.accent } else { c.faint }),
        );
    });

    // non-blocking notice: the engine thinks a bigger buffer would help
    if let Some(frames) = live.stats.suggest_buffer_frames
        && !core.notice_dismissed
    {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Audio is struggling at {} frames. Use a larger buffer ({frames})?",
                    live.stats.buffer_frames
                ))
                .color(c.warn),
            );
            if btn(ui, &format!("Use {frames}")).clicked() {
                core.set_buffer(frames);
            }
            if btn(ui, "Dismiss").clicked() {
                core.notice_dismissed = true;
            }
        });
    }
}

/// The tempo LCD: drag to change, double-click to type. Committed when released or on Enter.
fn tempo_lcd(ui: &mut Ui, core: &mut AppCore, theme: &Theme, bpm: f64) {
    let c = &theme.colors;
    let id = ui.id().with("tempo");
    let edit_id = id.with("editing");
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(104.0, 38.0), Sense::click_and_drag());
    let p = ui.painter();
    paint::well(p, rect, 3, c.lcd, theme);
    caption(
        p,
        rect.left_top() + Vec2::new(6.0, 3.0),
        "Tempo  BPM",
        theme,
    );
    let mut v: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(bpm);
    let editing: bool = ui.data(|d| d.get_temp(edit_id)).unwrap_or(false);
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.01
        } else {
            0.25
        };
        v = (v - resp.drag_delta().y as f64 * fine).clamp(10.0, 999.0);
        ui.data_mut(|d| d.insert_temp(id, v));
    }
    if resp.double_clicked() {
        ui.data_mut(|d| d.insert_temp(edit_id, true));
    }
    let mut commit = resp.drag_stopped();
    if editing {
        let mut text: String = ui
            .data(|d| d.get_temp(id.with("text")))
            .unwrap_or_else(|| format!("{v:.2}"));
        let inner = Rect::from_min_max(
            rect.left_top() + Vec2::new(6.0, 13.0),
            rect.right_bottom() - Vec2::new(6.0, 3.0),
        );
        let r = ui.put(
            inner,
            egui::TextEdit::singleline(&mut text)
                .font(FontId::new(
                    theme.metrics.font_lcd - 4.0,
                    crate::fonts::mono(),
                ))
                .text_color(c.accent)
                .frame(egui::Frame::NONE),
        );
        r.request_focus();
        ui.data_mut(|d| d.insert_temp(id.with("text"), text.clone()));
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if enter || r.lost_focus() || esc {
            if !esc && let Ok(t) = text.trim().parse::<f64>() {
                v = t.clamp(10.0, 999.0);
                commit = true;
            }
            ui.data_mut(|d| {
                d.remove::<bool>(edit_id);
                d.remove::<String>(id.with("text"));
            });
            if esc {
                ui.data_mut(|d| d.remove::<f64>(id));
            }
        }
    } else {
        paint::lcd_text(
            p,
            Pos2::new(rect.center().x, rect.bottom() - 15.0),
            Align2::CENTER_CENTER,
            &format!("{v:.2}"),
            FontId::new(theme.metrics.font_lcd - 4.0, crate::fonts::mono()),
            c.accent,
            c.glow_soft,
        );
    }
    resp.on_hover_text("Drag or double-click to type a tempo")
        .hint(
            "Tempo",
            &format!("{v:.2} BPM"),
            "drag (Shift = fine), double-click to type",
        );
    if commit {
        ui.data_mut(|d| d.remove::<f64>(id));
        if (v - bpm).abs() > 1e-9 {
            core.send(mapping::set_tempo(v));
        }
    }
}

/// The large bar : beat : tick readout with the clock time beside it.
fn position_lcd(ui: &mut Ui, theme: &Theme, pos: Tick, ticks_per_bar: Tick, bpm: f64) {
    let c = &theme.colors;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(214.0, 38.0), Sense::hover());
    let p = ui.painter();
    paint::well(p, rect, 3, c.lcd, theme);
    caption(
        p,
        rect.left_top() + Vec2::new(6.0, 3.0),
        "Bar  Beat  Tick",
        theme,
    );
    paint::lcd_text(
        p,
        Pos2::new(rect.left() + 10.0, rect.bottom() - 15.0),
        Align2::LEFT_CENTER,
        &format_position(pos, ticks_per_bar),
        FontId::new(theme.metrics.font_lcd, crate::fonts::mono()),
        c.accent,
        c.glow_soft,
    );
    p.text(
        Pos2::new(rect.right() - 8.0, rect.bottom() - 9.0),
        Align2::RIGHT_CENTER,
        format_time(pos, bpm),
        FontId::new(11.0, crate::fonts::mono()),
        c.dim,
    );
    resp.hint(
        "Position",
        &format!(
            "{}  {}",
            format_position(pos, ticks_per_bar),
            format_time(pos, bpm)
        ),
        "bar : beat : tick, and minutes : seconds",
    );
}

/// A small scope of the master level: a mirrored trace, newest on the right. (The engine
/// publishes levels, not samples, so this is an envelope scope.)
fn scope(ui: &mut Ui, theme: &Theme, trace: &[f32]) {
    let c = &theme.colors;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(120.0, 38.0), Sense::hover());
    let p = ui.painter();
    paint::well(p, rect, 3, c.lcd, theme);
    let mid = rect.center().y;
    p.line_segment(
        [
            Pos2::new(rect.left() + 3.0, mid),
            Pos2::new(rect.right() - 3.0, mid),
        ],
        Stroke::new(1.0, c.line_soft),
    );
    let inner = rect.shrink2(Vec2::new(3.0, 3.0));
    let n = crate::live::SCOPE_LEN;
    let dx = inner.width() / n as f32;
    let mut mesh = egui::Mesh::default();
    let start = n.saturating_sub(trace.len());
    for (i, v) in trace.iter().enumerate() {
        let h = widgets::meter_norm(*v) * inner.height() * 0.5;
        if h < 0.5 {
            continue;
        }
        let x = inner.left() + (start + i) as f32 * dx;
        let col = if *v > 1.0 { c.clip } else { c.accent };
        mesh.add_colored_rect(
            Rect::from_min_max(Pos2::new(x, mid - h), Pos2::new(x + dx.max(1.0), mid + h)),
            col.gamma_multiply(0.35 + 0.65 * i as f32 / trace.len().max(1) as f32),
        );
    }
    p.add(egui::Shape::mesh(mesh));
    p.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, c.line_soft),
        StrokeKind::Inside,
    );
    resp.hint("Scope", "", "the master level over the last few seconds");
}

/// DSP load and RAM, with the agent badge: the right end of the menu row.
pub fn health(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let c = &theme.colors;
    let live = core.live();
    // right to left: agent badge, RAM, DSP
    let agents = core.hooks.agents.clone();
    let live_agents: Vec<String> = agents
        .map(|d| {
            d.clients()
                .into_iter()
                .filter(|c| c.live)
                .map(|c| c.name)
                .collect()
        })
        .unwrap_or_default();
    let r = if live_agents.is_empty() {
        ui.label(widgets::caption(&theme, "Agent  idle"))
    } else {
        let text = if live_agents.len() == 1 {
            format!("Agent  {}", live_agents[0])
        } else {
            format!("Agents  {}", live_agents.len())
        };
        ui.scope(|ui| widgets::badge(ui, &text, c.agent, &theme))
            .response
    };
    r.hint(
        "Agent badge",
        &if live_agents.is_empty() {
            "no agent connected".to_string()
        } else {
            live_agents.join(", ")
        },
        "agents connect over MCP; see the Agents window",
    );
    ui.add_space(10.0);
    let mono = |t: String, col| {
        RichText::new(t)
            .font(FontId::new(11.5, crate::fonts::mono()))
            .color(col)
    };
    let ram = core.ram_bytes.map_or("--".to_string(), |b| {
        format!("{:.0} MB", b as f64 / 1_048_576.0)
    });
    ui.label(mono(format!("RAM {ram}"), c.dim))
        .hint("Memory", &ram, "memory used by Patternhaus");
    ui.add_space(8.0);
    let load = live.stats.dsp_load;
    let (kind, col) = if load > 0.8 {
        (widgets::Led::Clip, c.clip)
    } else if load > 0.5 {
        (widgets::Led::Warn, c.warn)
    } else {
        (widgets::Led::On, c.dim)
    };
    ui.label(mono(format!("DSP {:.0}%", load * 100.0), col))
        .on_hover_text(format!(
            "{} frames at {} Hz",
            live.stats.buffer_frames, live.stats.sample_rate
        ))
        .hint(
            "DSP load",
            &format!("{:.0}%", load * 100.0),
            "audio processing time as a share of the buffer",
        );
    widgets::led(ui, kind, "Audio engine", &theme);
    if live.stats.overloads > 0 {
        ui.label(mono(format!("{} overloads", live.stats.overloads), c.error));
    }
    if let Some(n) = &core.audio_notice {
        ui.label(RichText::new(n).color(c.warn));
    }
}

/// Opens the export dialog (also reachable from the toolbar and the File menu).
pub fn open_export(core: &mut AppCore) {
    core.dialog = Some(Dialog::Export {
        path: "exports/mix.wav".into(),
        overwrite: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_formats_as_bar_beat_tick() {
        assert_eq!(format_position(0, 3840), "001:1:000");
        assert_eq!(format_position(960, 3840), "001:2:000");
        assert_eq!(format_position(3840 + 960 * 2 + 17, 3840), "002:3:017");
    }

    #[test]
    fn time_formats_minutes_and_seconds() {
        // 120 bpm: a quarter note is 0.5 s
        assert_eq!(format_time(960 * 2, 120.0), "00:01.0");
        assert_eq!(format_time(960 * 2 * 61, 120.0), "01:01.0");
    }
}
