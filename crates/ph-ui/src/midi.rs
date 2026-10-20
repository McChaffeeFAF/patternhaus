//! MIDI controllers in the UI (M2/T8): MIDI learn on any knob, mapped controllers moving
//! knobs, and the selected channel receiving the keyboard.
//!
//! The devices and the router are in `ph_engine::midi`; this is the glue.
//!
//! - **Learn.** A control's right-click menu (next to "Create automation clip") asks to learn
//!   its target. Controls cannot reach `AppCore`, so like automation the request travels
//!   through the egui context and the core drains it each frame ([`take_requests`]) and sends
//!   the `midi.learn` / `midi.unmap` commands. The core publishes a [`MidiView`] (what is
//!   learning, what is mapped) for the menus.
//! - **Moving knobs.** A mapped controller arrives as a `MidiEvent::Control` with a 0..1 value.
//!   The core scales it into the target's range ([`plain_value`]) and sends the same command a
//!   knob drag sends ([`crate::mapping::set_target`]), at most once per target per frame, so
//!   the engine, undo and agents see an ordinary edit. With automation recording armed and the
//!   song playing, the move is also collected for `automation.record`, like a drag.
//! - **The keyboard.** Notes, bend and unmapped controllers go to the selected channel; the
//!   core tells the service which one each frame.

use crate::core::{AppCore, ToastKind};
use crate::mapping;
use egui::Id;
use ph_core::{ParamPath, Project};
use ph_engine::midi::MidiEvent;
use std::collections::BTreeMap;
use std::sync::Arc;

const VIEW: &str = "ph_midi_view";
const REQUESTS: &str = "ph_midi_requests";

/// What the menus may know about MIDI this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiView {
    /// The target waiting for a controller to be moved.
    pub learning: Option<ParamPath>,
    /// Controllers mapped to each target: (controller, device).
    pub mapped: BTreeMap<ParamPath, Vec<(u8, Option<String>)>>,
}

/// A control's menu asked for this.
#[derive(Clone, Debug, PartialEq)]
pub enum MidiRequest {
    Learn(ParamPath),
    CancelLearn,
    Unmap {
        controller: u8,
        device: Option<String>,
    },
}

pub fn publish(ctx: &egui::Context, view: MidiView) {
    ctx.data_mut(|d| d.insert_temp(Id::new(VIEW), Arc::new(view)));
}

pub fn view(ctx: &egui::Context) -> Option<Arc<MidiView>> {
    ctx.data(|d| d.get_temp::<Arc<MidiView>>(Id::new(VIEW)))
}

pub fn request(ctx: &egui::Context, r: MidiRequest) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<MidiRequest>>(Id::new(REQUESTS))
            .push(r)
    });
}

pub fn take_requests(ctx: &egui::Context) -> Vec<MidiRequest> {
    ctx.data_mut(|d| d.remove_temp::<Vec<MidiRequest>>(Id::new(REQUESTS)))
        .unwrap_or_default()
}

/// A 0..1 controller value as the target's own value: linear over its range (the knobs are
/// linear too), whole numbers for stepped parameters.
pub fn plain_value(project: &Project, target: &ParamPath, norm: f64) -> Option<f64> {
    let norm = norm.clamp(0.0, 1.0);
    if let Some((lo, hi)) = target.fixed_range() {
        let v = lo + norm * (hi - lo);
        // a mute is a switch
        return Some(if matches!(target, ParamPath::InsertMute(_)) {
            if norm >= 0.5 { 1.0 } else { 0.0 }
        } else {
            v
        });
    }
    let info = crate::automation::param_info(project, target);
    match info {
        Some(i) if i.max > i.min => {
            let v = i.min + norm * (i.max - i.min);
            Some(if i.stepped { v.round() } else { v })
        }
        // a plugin's parameter: the range lives in the plugin; the model keeps plain values
        // the host clamps, so map onto 0..1 as the plugin API does
        _ => matches!(
            target,
            ParamPath::ChannelParam(..) | ParamPath::EffectParam { .. }
        )
        .then_some(norm),
    }
}

/// Per-core state.
#[derive(Default)]
pub struct MidiUi {
    /// The channel last told to the service.
    told: Option<Option<ph_core::ChannelId>>,
    /// Waker installed.
    waker: bool,
    /// Last value sent per target, to skip repeats.
    sent: BTreeMap<ParamPath, f64>,
    /// Time of the last note (activity light).
    pub last_note_at: f64,
}

impl AppCore {
    /// MIDI, once a frame: the keyboard's channel, learn requests from menus, the events from
    /// the router (mapped controllers move knobs), and the view the menus read.
    pub fn update_midi(&mut self, ctx: &egui::Context) {
        let midi = self.studio.midi.clone();
        if !self.midi.waker {
            self.midi.waker = true;
            let c = ctx.clone();
            midi.set_waker(Some(Arc::new(move || c.request_repaint())));
        }
        // a finished take opens in the audio editor
        if let Some(s) = self.rec.open_take
            && self.project.samples.contains_key(&s)
        {
            self.rec.open_take = None;
            self.window_requests.push((
                crate::workspace::WindowId::AudioEditor(s),
                crate::core::PanelAction::Open,
            ));
        }
        let want = self.sel.channel;
        if self.midi.told != Some(want) {
            self.midi.told = Some(want);
            midi.set_target_channel(want);
        }
        for r in take_requests(ctx) {
            match r {
                MidiRequest::Learn(t) => {
                    self.toast(
                        ToastKind::Info,
                        format!(
                            "Move a knob or slider on your MIDI device to map it to {}",
                            crate::automation::target_name(&self.project, &t)
                        ),
                    );
                    self.send(mapping::midi_learn(Some(&t)));
                }
                MidiRequest::CancelLearn => self.send(mapping::midi_learn(None)),
                MidiRequest::Unmap { controller, device } => {
                    self.send(mapping::midi_unmap(controller, device.as_deref()))
                }
            }
        }
        // events: the last controller value per target wins within a frame
        let mut moves: BTreeMap<ParamPath, f64> = BTreeMap::new();
        for e in midi.drain_events() {
            match e {
                MidiEvent::Control { target, value, .. } => {
                    moves.insert(target, value);
                }
                MidiEvent::Learned {
                    controller,
                    device,
                    target,
                } => self.toast(
                    ToastKind::Info,
                    format!(
                        "CC {controller} on {device} now moves {}",
                        crate::automation::target_name(&self.project, &target)
                    ),
                ),
                MidiEvent::Devices => {}
                MidiEvent::Note { on: true, .. } => self.midi.last_note_at = self.now,
                MidiEvent::Note { .. } => {}
            }
        }
        let live = self.link.snapshot();
        for (target, norm) in moves {
            let Some(plain) = plain_value(&self.project, &target, norm) else {
                continue;
            };
            let model = target.model_value(&self.project);
            let last = self.midi.sent.get(&target).copied().or(model);
            if last.is_some_and(|l| (l - plain).abs() < 1e-9) {
                continue;
            }
            let Some(req) = mapping::set_target(&self.project, &target, plain) else {
                continue;
            };
            self.midi.sent.insert(target.clone(), plain);
            self.send(req);
            if self.auto_armed && live.status.playing {
                self.auto_rec
                    .entry(target)
                    .or_default()
                    .push((live.status.position, plain));
            }
        }
        // the menus' view
        let mut mapped: BTreeMap<ParamPath, Vec<(u8, Option<String>)>> = BTreeMap::new();
        for m in midi.mappings() {
            mapped
                .entry(m.target.clone())
                .or_default()
                .push((m.controller, m.device.clone()));
        }
        publish(
            ctx,
            MidiView {
                learning: midi.learning(),
                mapped,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_core::{ChannelId, Edit, InsertId};

    #[test]
    fn controller_values_scale_into_the_targets_range() {
        let p = Project::empty("t");
        let ch = ChannelId(0);
        assert_eq!(
            plain_value(&p, &ParamPath::ChannelVolume(ch), 0.5),
            Some(0.5)
        );
        assert_eq!(plain_value(&p, &ParamPath::ChannelPan(ch), 0.0), Some(-1.0));
        assert_eq!(plain_value(&p, &ParamPath::ChannelPan(ch), 1.0), Some(1.0));
        assert_eq!(
            plain_value(&p, &ParamPath::InsertVolume(InsertId(1)), 1.0),
            Some(2.0)
        );
        assert_eq!(plain_value(&p, &ParamPath::Tempo, 0.0), Some(10.0));
        assert_eq!(plain_value(&p, &ParamPath::Tempo, 1.0), Some(999.0));
        // out of range input is clamped
        assert_eq!(
            plain_value(&p, &ParamPath::ChannelVolume(ch), 7.0),
            Some(1.0)
        );
        // a mute is a switch at the halfway point
        let mute = ParamPath::InsertMute(InsertId(1));
        assert_eq!(plain_value(&p, &mute, 0.4), Some(0.0));
        assert_eq!(plain_value(&p, &mute, 0.6), Some(1.0));
    }

    #[test]
    fn processor_parameters_use_their_descriptor_range() {
        let mut e = Edit::new(&Project::empty("t"));
        let c = e
            .add_channel(
                "Keys",
                ph_core::InstrumentSpec::Synth {
                    preset: None,
                    params: Default::default(),
                },
            )
            .unwrap();
        let p = e.finish().0;
        let t = ParamPath::ChannelParam(c, "f1_cutoff".into());
        let info = crate::automation::param_info(&p, &t).expect("the synth has a cutoff");
        assert_eq!(plain_value(&p, &t, 0.0), Some(info.min));
        assert_eq!(plain_value(&p, &t, 1.0), Some(info.max));
        // a parameter that is not there maps onto 0..1
        let gone = ParamPath::ChannelParam(c, "nope".into());
        assert_eq!(plain_value(&p, &gone, 0.25), Some(0.25));
        // a channel that is not there is nothing to move
        let none = ParamPath::ChannelVolume(ChannelId(9));
        assert!(mapping::set_target(&p, &none, 0.5).is_none());
    }
}
