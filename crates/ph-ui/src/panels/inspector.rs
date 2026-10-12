//! Instrument panel for the selected channel: Sampler with a waveform view, Haus Synth grouped
//! by section. Knobs are generated from the engine's parameter descriptors.

use crate::browser_model::{Dragged, Kind};
use crate::core::AppCore;
use crate::hint::HintExt;
use crate::mapping;
use crate::theme::{Theme, rgb};
use crate::widgets::{self, param_grid};
use egui::{CollapsingHeader, CornerRadius, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use ph_core::{ChannelId, InstrumentSpec};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WAVE_COLUMNS: usize = 900;
const MAX_PREVIEW_BYTES: u64 = 24 * 1024 * 1024;

/// Min/max peaks per column of a decoded sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    pub peaks: Vec<(f32, f32)>,
    pub frames: usize,
    pub sample_rate: f64,
}

#[derive(Default)]
pub struct InspectorState {
    waves: HashMap<String, Option<Waveform>>,
}

impl InspectorState {
    /// The (cached) waveform of a sample reference, for the browser's preview.
    pub fn wave(
        &mut self,
        reference: &str,
        assets_dir: &Path,
        project_dir: &Path,
    ) -> Option<Waveform> {
        self.waves
            .entry(reference.to_string())
            .or_insert_with(|| {
                load_waveform(&resolve_reference(reference, assets_dir, project_dir))
            })
            .clone()
    }
}

/// Where a sample reference lives on disk.
pub fn resolve_reference(reference: &str, assets_dir: &Path, project_dir: &Path) -> PathBuf {
    if let Some(rest) = reference.strip_prefix("factory:") {
        assets_dir.join("samples").join(format!("{rest}.wav"))
    } else {
        let p = Path::new(reference);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_dir.join(p)
        }
    }
}

/// Reduces channels to a mono mix and then to `columns` min/max pairs.
pub fn peaks_of(channels: &[Vec<f32>], columns: usize) -> Vec<(f32, f32)> {
    let frames = channels.first().map_or(0, |c| c.len());
    if frames == 0 || columns == 0 {
        return Vec::new();
    }
    let n = channels.len() as f32;
    (0..columns)
        .map(|c| {
            let a = c * frames / columns;
            let b = (((c + 1) * frames) / columns).max(a + 1).min(frames);
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for f in a..b {
                let v = channels.iter().map(|ch| ch[f]).sum::<f32>() / n;
                lo = lo.min(v);
                hi = hi.max(v);
            }
            (lo, hi)
        })
        .collect()
}

fn load_waveform(path: &Path) -> Option<Waveform> {
    if std::fs::metadata(path).ok()?.len() > MAX_PREVIEW_BYTES {
        return None;
    }
    let d = ph_formats::decode::decode_file(path).ok()?;
    Some(Waveform {
        peaks: peaks_of(&d.channels, WAVE_COLUMNS),
        frames: d.frames(),
        sample_rate: d.sample_rate,
    })
}

/// Which section of the synth a parameter id belongs to.
pub fn synth_section(id: &str) -> &'static str {
    let p = id.split('_').next().unwrap_or(id);
    match p {
        "osc1" | "osc2" | "osc3" => "Oscillators",
        "sub" | "noise" => "Sub and noise",
        "f1" | "f2" | "filter" => "Filters",
        "amp" | "fenv" | "menv" => "Envelopes",
        "lfo1" | "lfo2" => "LFOs",
        "mod" if id.starts_with("mod_wheel") => "Voice and output",
        "mod1" | "mod2" | "mod3" | "mod4" | "mod5" | "mod6" | "mod7" | "mod8" => {
            "Modulation matrix"
        }
        _ => "Voice and output",
    }
}

pub const SYNTH_SECTIONS: [&str; 7] = [
    "Oscillators",
    "Sub and noise",
    "Filters",
    "Envelopes",
    "LFOs",
    "Modulation matrix",
    "Voice and output",
];

/// The instrument window of one channel (`WindowId::Instrument`).
pub fn show(ui: &mut Ui, core: &mut AppCore, channel: ChannelId) {
    let theme = core.theme.clone();
    let project = core.project.clone();
    let Some(ch) = project.channels.get(&channel).cloned() else {
        ui.label("This channel no longer exists.");
        return;
    };
    ui.horizontal(|ui| {
        let (r, p) = ui.allocate_painter(Vec2::new(8.0, 20.0), Sense::hover());
        p.rect_filled(r.rect, CornerRadius::same(2), rgb(ch.color));
        crate::widgets::leave_room(ui, 110.0, |ui| {
            ui.add(egui::Label::new(RichText::new(&ch.name).strong().size(15.0)).truncate())
        });
        ui.label(
            RichText::new(match &ch.instrument {
                InstrumentSpec::Sampler { .. } => "Sampler",
                InstrumentSpec::Synth { .. } => "Haus Synth",
                InstrumentSpec::MultiSampler { .. } => "Haus Sampler",
                InstrumentSpec::Plugin { .. } => "Plugin",
            })
            .color(theme.colors.dim),
        );
    });
    ui.separator();
    let panel_rect = ui.available_rect_before_wrap();
    let desc = ph_engine::catalog::instrument_descriptor(&ch.instrument);
    // the window itself scrolls (see `windows::show_content`)
    match &ch.instrument {
        InstrumentSpec::Sampler {
            sample,
            root_key,
            params,
        } => {
            sampler(
                ui,
                core,
                &ch,
                sample.as_deref(),
                *root_key,
                params,
                desc.as_ref().ok(),
                &theme,
            );
        }
        InstrumentSpec::Synth { preset, params } => {
            synth(
                ui,
                core,
                &ch,
                preset.as_deref(),
                params,
                desc.as_ref().ok(),
                &theme,
            );
        }
        _ => {
            ui.label("No panel for this instrument type yet.");
        }
    }

    // dropping a sample on the panel replaces the sampler's sample
    if ui.input(|i| i.pointer.any_released())
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        && panel_rect.contains(pos)
        && let InstrumentSpec::Sampler {
            root_key, params, ..
        } = &ch.instrument
        && let Some(d) = egui::DragAndDrop::take_payload::<Dragged>(ui.ctx())
        && let Kind::Sample { reference } = &d.kind
    {
        let inst = InstrumentSpec::Sampler {
            sample: Some(reference.clone()),
            root_key: *root_key,
            params: params.clone(),
        };
        core.send(mapping::channel_set_instrument(ch.id, &inst));
    }
}

#[allow(clippy::too_many_arguments)]
fn sampler(
    ui: &mut Ui,
    core: &mut AppCore,
    ch: &ph_core::Channel,
    sample: Option<&str>,
    root_key: u8,
    params: &std::collections::BTreeMap<String, f64>,
    desc: Option<&ph_node::NodeDescriptor>,
    theme: &Theme,
) {
    crate::widgets::leave_room(ui, 0.0, |ui| {
        ui.add(
            egui::Label::new(format!(
                "Sample: {}",
                sample.unwrap_or("(none) - drop one here from the browser")
            ))
            .truncate(),
        )
    });
    if let Some(r) = sample {
        let (assets, project_dir) = (
            core.paths.assets_dir.clone(),
            core.paths.project_dir.clone(),
        );
        let wave = core
            .inspector
            .waves
            .entry(r.to_string())
            .or_insert_with(|| load_waveform(&resolve_reference(r, &assets, &project_dir)))
            .clone();
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().max(200.0), 110.0),
            Sense::hover(),
        );
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(4), theme.colors.bg0);
        match wave {
            Some(w) => {
                let mid = rect.center().y;
                let half = rect.height() * 0.5 - 4.0;
                let mut mesh = egui::Mesh::default();
                for (i, (lo, hi)) in w.peaks.iter().enumerate() {
                    let x = rect.left() + (i as f32 + 0.5) / w.peaks.len() as f32 * rect.width();
                    mesh.add_colored_rect(
                        Rect::from_min_max(
                            Pos2::new(x - 0.5, mid - hi.max(0.004) * half),
                            Pos2::new(x + 0.5, mid - lo.min(-0.004) * half),
                        ),
                        rgb(ch.color),
                    );
                }
                p.add(egui::Shape::mesh(mesh));
                p.line_segment(
                    [Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)],
                    Stroke::new(1.0, theme.colors.grid_major),
                );
                // start / end markers from the params
                let start = params.get("start").copied().unwrap_or(0.0) as f32;
                let end = params.get("end").copied().unwrap_or(1.0) as f32;
                for (t, label) in [(start, "start"), (end, "end")] {
                    let x = rect.left() + rect.width() * t;
                    p.line_segment(
                        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                        Stroke::new(1.5, theme.colors.accent),
                    );
                    p.text(
                        Pos2::new(x + 3.0, rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::proportional(9.0),
                        theme.colors.accent,
                    );
                }
                p.text(
                    rect.right_bottom() - Vec2::new(4.0, 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    format!("{:.2} s", w.frames as f64 / w.sample_rate.max(1.0)),
                    egui::FontId::proportional(10.0),
                    theme.colors.dim,
                );
            }
            None => {
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "(cannot preview this file)",
                    egui::FontId::proportional(12.0),
                    theme.colors.dim,
                );
            }
        }
    }
    ui.horizontal(|ui| {
        ui.label("Root key");
        let mut rk = root_key as i32;
        let r = ui
            .add(
                egui::DragValue::new(&mut rk)
                    .range(0..=127)
                    .custom_formatter(|v, _| crate::piano_roll::note_name(v as u8)),
            )
            .hint_menu(
                "Root key",
                &crate::piano_roll::note_name(rk as u8),
                "drag or type: the key the sample plays at its recorded pitch",
            );
        if (r.drag_stopped() || r.lost_focus()) && rk != root_key as i32 {
            let inst = InstrumentSpec::Sampler {
                sample: sample.map(str::to_string),
                root_key: rk as u8,
                params: params.clone(),
            };
            core.send(mapping::channel_set_instrument(ch.id, &inst));
        }
    });
    if let Some(d) = desc {
        let changes = param_grid(
            ui,
            Id::new(("smp", ch.id)),
            d,
            &|p| p.id != "root_key",
            &|p| params.get(&p.id).copied().unwrap_or(p.default),
            theme,
        );
        for (id, v) in changes {
            core.send(mapping::channel_param(ch.id, &id, v));
        }
    }
}

fn synth(
    ui: &mut Ui,
    core: &mut AppCore,
    ch: &ph_core::Channel,
    preset: Option<&str>,
    params: &std::collections::BTreeMap<String, f64>,
    desc: Option<&ph_node::NodeDescriptor>,
    theme: &Theme,
) {
    ui.horizontal(|ui| {
        ui.label("Preset");
        egui::ComboBox::from_id_salt(("preset", ch.id))
            .selected_text(preset.unwrap_or("(init)"))
            .width(180.0)
            .show_ui(ui, |ui| {
                for name in ph_engine::catalog::synth_presets() {
                    if ui.selectable_label(preset == Some(name), name).clicked() {
                        let inst = InstrumentSpec::Synth {
                            preset: Some(name.to_string()),
                            params: Default::default(),
                        };
                        core.send(mapping::channel_set_instrument(ch.id, &inst));
                    }
                }
            })
            .response
            .hint_menu(
                "Synth preset",
                preset.unwrap_or("(init)"),
                "click to load a preset into this channel",
            );
        if !params.is_empty() {
            ui.label(RichText::new(format!("{} edited", params.len())).color(theme.colors.dim));
        }
    });
    let Some(d) = desc else { return };
    for section in SYNTH_SECTIONS {
        CollapsingHeader::new(crate::theme::caps(section))
            .id_salt(("synth_sec", ch.id, section))
            .default_open(matches!(section, "Oscillators" | "Filters" | "Envelopes"))
            .show(ui, |ui| {
                let changes = param_grid(
                    ui,
                    Id::new(("syn", ch.id, section)),
                    d,
                    &|p| synth_section(&p.id) == section,
                    &|p| params.get(&p.id).copied().unwrap_or(p.default),
                    theme,
                );
                for (id, v) in changes {
                    core.send(mapping::channel_param(ch.id, &id, v));
                }
            })
            .header_response
            .hint(
                section,
                "",
                "click to fold or unfold this group of controls",
            );
    }
    let _ = widgets::Edit::None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_resolve_to_paths() {
        let a = Path::new("/assets");
        let p = Path::new("/proj");
        assert_eq!(
            resolve_reference("factory:drums/kick", a, p),
            Path::new("/assets/samples/drums/kick.wav")
        );
        assert_eq!(
            resolve_reference("samples/x.wav", a, p),
            Path::new("/proj/samples/x.wav")
        );
    }

    #[test]
    fn peaks_cover_the_whole_sample() {
        let mut ch = vec![0.0f32; 1000];
        ch[10] = 0.5;
        ch[990] = -0.75;
        let p = peaks_of(&[ch], 10);
        assert_eq!(p.len(), 10);
        assert_eq!(p[0].1, 0.5);
        assert_eq!(p[9].0, -0.75);
        assert!(peaks_of(&[], 10).is_empty());
    }

    #[test]
    fn synth_params_all_land_in_a_known_section() {
        let d = ph_engine::catalog::instrument_descriptor(&InstrumentSpec::Synth {
            preset: None,
            params: Default::default(),
        })
        .unwrap();
        for p in &d.params {
            assert!(SYNTH_SECTIONS.contains(&synth_section(&p.id)), "{}", p.id);
        }
        assert_eq!(synth_section("osc2_wave"), "Oscillators");
        assert_eq!(synth_section("f1_cutoff"), "Filters");
        assert_eq!(synth_section("mod3_amt"), "Modulation matrix");
        assert_eq!(synth_section("mod_wheel"), "Voice and output");
        assert_eq!(synth_section("volume"), "Voice and output");
    }

    #[test]
    fn real_factory_sample_has_a_waveform() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let p = resolve_reference("factory:drums/haus808/kick", &assets, Path::new("."));
        let w = load_waveform(&p).expect("decodes");
        assert_eq!(w.peaks.len(), WAVE_COLUMNS);
        assert!(w.peaks.iter().any(|(lo, hi)| hi - lo > 0.1));
    }
}
