//! Recording in the transport row (M2/T8): the REC toggle and the audio inputs popup.
//!
//! - **REC** sends `record.start` (with the count-in and punch settings below) or
//!   `record.stop`. It records every armed audio input and, in pattern mode, the MIDI notes
//!   played on the selected channel. The Stop button ends a recording too, through
//!   `record.stop`, so the takes always reach the project.
//! - **IN** opens the popup: the audio inputs (from `record.list_inputs`), each with an arm
//!   toggle, a monitor toggle and a level; where armed inputs record (a playlist track or a
//!   Sampler channel); the count-in and punch-to-loop settings; the latency readout; and the
//!   MIDI devices.
//!
//! Everything goes through commands. The UI never touches a device: the input list comes back
//! as a command result, and the levels and phase come from the engine poller (`Live`).

use crate::core::{AppCore, ToastKind};
use crate::hint::HintExt;
use crate::mapping;
use crate::widgets::{self, btn, toggle};
use egui::{Color32, RichText, Sense, Ui, Vec2};
use ph_commands::{LatencyInfo, RecordInput, RecordInputs};
use ph_core::ChannelId;
use ph_engine::record::RecordPhase;

/// Count-in lengths the popup offers, in bars.
pub const COUNT_INS: [u32; 4] = [0, 1, 2, 4];

#[derive(Clone, Debug, Default)]
pub struct RecorderUi {
    /// A recording this UI started is running (cleared when `record.stop` returns or
    /// `record.start` fails).
    pub active: bool,
    /// The inputs as the last `record.list_inputs` reported them.
    pub inputs: Vec<RecordInput>,
    pub latency: Option<LatencyInfo>,
    /// Bars of count-in.
    pub count_in: u32,
    /// Record only inside the loop region.
    pub punch_to_loop: bool,
    /// Armed inputs record into a Sampler channel instead of a playlist track.
    pub to_channel: bool,
    /// The playlist track for a new take (0-based).
    pub track: u32,
    pub channel: Option<ChannelId>,
    /// The popup has asked for the inputs since it opened.
    asked: bool,
}

impl RecorderUi {
    /// Takes a `record.list_inputs` result.
    pub fn on_inputs(&mut self, result: &serde_json::Value) {
        if let Ok(v) = serde_json::from_value::<RecordInputs>(result.clone()) {
            self.inputs = v.inputs;
            self.latency = Some(v.latency);
        }
    }
}

impl AppCore {
    /// Starts or stops recording, as the REC button does.
    pub fn toggle_recording(&mut self) {
        let live = self.link.snapshot();
        if self.rec.active || live.recording {
            self.rec.active = false;
            self.send(mapping::record_stop());
        } else {
            let punch = if self.rec.punch_to_loop {
                live.status.loop_range
            } else {
                None
            };
            self.rec.active = true;
            self.send(mapping::record_start(self.rec.count_in, punch));
        }
    }

    /// A recording reply arrived: refresh the inputs after arming changes, tell the person
    /// what was recorded, and let go of the record state when a start failed.
    pub fn on_record_reply(&mut self, command: &str, ok: Option<&ph_commands::Executed>) {
        match (command, ok) {
            ("record.start", None) => self.rec.active = false,
            ("record.list_inputs", Some(ex)) => self.rec.on_inputs(&ex.result),
            ("record.arm" | "record.disarm" | "record.set_monitor", Some(_)) => {
                self.send(mapping::record_list_inputs())
            }
            ("record.stop", Some(ex)) => {
                self.rec.active = false;
                if let Some(s) = &ex.summary {
                    self.toast(ToastKind::Info, s.clone());
                }
                for w in ex.result["warnings"].as_array().into_iter().flatten() {
                    if let Some(w) = w.as_str() {
                        self.toast(ToastKind::Warn, w.to_string());
                    }
                }
            }
            ("record.stop", None) => self.rec.active = false,
            _ => {}
        }
    }
}

/// The REC toggle and the IN button, in the transport row.
pub fn show(ui: &mut Ui, core: &mut AppCore) {
    let theme = core.theme.clone();
    let c = &theme.colors;
    let live = core.live();
    let recording = core.rec.active || live.recording;
    let (label, tip) = match live.record.phase {
        RecordPhase::CountIn => ("REC", "counting in; click to cancel and stop"),
        _ => (
            "REC",
            "records the armed audio inputs, and the MIDI notes you play in pattern mode",
        ),
    };
    if toggle(ui, recording, label, c.error, &theme)
        .on_hover_text(tip)
        .hint(
            "Record",
            if recording { "recording" } else { "" },
            "arm an input under IN first; click again to stop",
        )
        .clicked()
    {
        core.toggle_recording();
    }
    inputs_button(ui, core, &live);
}

fn inputs_button(ui: &mut Ui, core: &mut AppCore, live: &crate::live::Live) {
    let r = btn(ui, "IN");
    let r = r
        .on_hover_text("Audio inputs: arm, monitor, count-in, punch and latency")
        .hint(
            "Audio inputs",
            "",
            "arm an input to record it, hear it while you play",
        );
    let theme = core.theme.clone();
    let popup = egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let shown = popup.show(|ui| {
        ui.set_min_width(340.0);
        if !core.rec.asked_once() {
            core.rec.mark_asked();
            core.send(mapping::record_list_inputs());
        }
        inputs_popup(ui, core, live, &theme);
    });
    if shown.is_none() {
        core.rec.reset_asked();
    }
}

impl RecorderUi {
    fn asked_once(&self) -> bool {
        self.asked
    }
    fn mark_asked(&mut self) {
        self.asked = true;
    }
    fn reset_asked(&mut self) {
        self.asked = false;
    }
}

fn inputs_popup(ui: &mut Ui, core: &mut AppCore, live: &crate::live::Live, theme: &crate::theme::Theme) {
    let c = &theme.colors;
    ui.horizontal(|ui| {
        ui.label(widgets::caption(theme, "Audio inputs"));
        if btn(ui, "Refresh").clicked() {
            core.send(mapping::record_list_inputs());
        }
    });
    if core.rec.inputs.is_empty() {
        ui.label(
            RichText::new("No audio input found. Check the device in your system sound settings.")
                .color(c.dim),
        );
    }
    // where armed inputs record
    let tracks = core.project.tracks.len().max(1) as u32;
    let sampler_channels: Vec<(ChannelId, String)> = core
        .project
        .channels_in_order()
        .filter(|ch| matches!(ch.instrument, ph_core::InstrumentSpec::Sampler { .. }))
        .map(|ch| (ch.id, ch.name.clone()))
        .collect();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Record onto").color(c.dim));
        if ui
            .selectable_label(!core.rec.to_channel, "a playlist track")
            .clicked()
        {
            core.rec.to_channel = false;
        }
        if ui
            .add_enabled(
                !sampler_channels.is_empty(),
                egui::Button::selectable(core.rec.to_channel, "a Sampler channel"),
            )
            .clicked()
        {
            core.rec.to_channel = true;
            if core.rec.channel.is_none() {
                core.rec.channel = sampler_channels.first().map(|c| c.0);
            }
        }
    });
    if core.rec.to_channel {
        let cur = core
            .rec
            .channel
            .and_then(|id| sampler_channels.iter().find(|c| c.0 == id))
            .map_or("(none)".to_string(), |c| c.1.clone());
        egui::ComboBox::from_id_salt("rec_channel")
            .selected_text(cur)
            .show_ui(ui, |ui| {
                for (id, name) in &sampler_channels {
                    if ui
                        .selectable_label(core.rec.channel == Some(*id), name)
                        .clicked()
                    {
                        core.rec.channel = Some(*id);
                    }
                }
            });
    } else {
        core.rec.track = core.rec.track.min(tracks - 1);
        egui::ComboBox::from_id_salt("rec_track")
            .selected_text(format!("Track {}", core.rec.track + 1))
            .show_ui(ui, |ui| {
                for t in 0..tracks {
                    if ui
                        .selectable_label(core.rec.track == t, format!("Track {}", t + 1))
                        .clicked()
                    {
                        core.rec.track = t;
                    }
                }
            });
    }
    ui.separator();
    for input in core.rec.inputs.clone() {
        ui.horizontal(|ui| {
            let mut armed = input.armed;
            if ui
                .checkbox(&mut armed, RichText::new(&input.name))
                .on_hover_text("Arm to record this input")
                .changed()
            {
                if armed {
                    let (track, channel) = if core.rec.to_channel {
                        (None, core.rec.channel)
                    } else {
                        (Some(core.rec.track), None)
                    };
                    core.send(mapping::record_arm(input.id, track, channel, true));
                } else {
                    core.send(mapping::record_disarm(input.id));
                }
            }
        });
        if input.armed {
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                let mut monitor = input.monitor;
                if ui
                    .checkbox(&mut monitor, "Monitor")
                    .on_hover_text("Hear the input while you play, through its target")
                    .changed()
                {
                    core.send(mapping::record_monitor(input.id, monitor));
                }
                let level = live
                    .record
                    .inputs
                    .iter()
                    .find(|(id, _)| id.0 == input.id)
                    .map_or(0.0, |(_, p)| *p);
                level_bar(ui, level, theme);
                let target = match (input.track, input.channel) {
                    (Some(t), _) => format!("track {}", t + 1),
                    (None, Some(ch)) => core
                        .project
                        .channels
                        .get(&ch)
                        .map_or("a channel".into(), |c| c.name.clone()),
                    _ => String::new(),
                };
                ui.label(RichText::new(format!("-> {target}")).color(c.dim));
            });
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Count-in").color(c.dim));
        for bars in COUNT_INS {
            let label = if bars == 0 {
                "off".to_string()
            } else {
                format!("{bars} bar{}", if bars == 1 { "" } else { "s" })
            };
            if ui
                .selectable_label(core.rec.count_in == bars, label)
                .clicked()
            {
                core.rec.count_in = bars;
            }
        }
    });
    let has_loop = live.status.loop_range.is_some();
    ui.horizontal(|ui| {
        let mut punch = core.rec.punch_to_loop;
        if ui
            .add_enabled(has_loop, egui::Checkbox::new(&mut punch, "Punch in and out over the loop region"))
            .on_disabled_hover_text("Set a loop region first (shift-drag the playlist ruler)")
            .changed()
        {
            core.rec.punch_to_loop = punch;
        }
    });
    // the readout
    if let Some(l) = &core.rec.latency {
        let ms = |f: u32| f as f64 * 1000.0 / l.sample_rate.max(1) as f64;
        ui.label(
            RichText::new(format!(
                "Latency: input {:.1} ms, output {:.1} ms; hearing yourself {:.1} ms; takes are moved {:.1} ms earlier",
                ms(l.input_frames),
                ms(l.output_frames),
                l.monitor_ms,
                l.compensation_ms
            ))
            .size(11.5)
            .color(c.dim),
        );
    }
    ui.separator();
    ui.label(widgets::caption(theme, "MIDI devices"));
    let midi = core.studio.midi.inputs();
    if midi.is_empty() {
        ui.label(RichText::new("No MIDI device found.").color(c.dim));
    }
    for m in midi {
        ui.label(
            RichText::new(format!(
                "{} {}",
                if m.connected { "\u{25cf}" } else { "\u{25cb}" },
                m.name
            ))
            .color(if m.connected { c.text } else { c.dim }),
        );
    }
    let sel = core
        .sel
        .channel
        .and_then(|id| core.project.channels.get(&id))
        .map_or("(no channel selected)".to_string(), |c| c.name.clone());
    ui.label(
        RichText::new(format!("Keys play: {sel}"))
            .size(11.5)
            .color(c.dim),
    );
}

fn level_bar(ui: &mut Ui, level: f32, theme: &crate::theme::Theme) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(90.0, 8.0), Sense::hover());
    let c = &theme.colors;
    ui.painter().rect_filled(rect, 2.0, c.lcd);
    let w = rect.width() * widgets::meter_norm(level);
    let col: Color32 = if level >= 1.0 {
        c.clip
    } else if level > 0.7 {
        c.warn
    } else {
        c.accent
    };
    if w > 0.5 {
        let mut r = rect;
        r.set_width(w);
        ui.painter().rect_filled(r, 2.0, col);
    }
    resp.hint("Input level", "", "the armed input's peak");
}
