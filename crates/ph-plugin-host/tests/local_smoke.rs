//! A local smoke test over installed plugins (never in CI: `#[ignore]` and it needs
//! `PH_PLUGIN_SMOKE`, a `;`-separated list of plugin files or bundles). For each plugin it
//! creates an instance, activates it at 48 kHz / 256 frames, processes two seconds with a few
//! notes, reads parameters, and saves and reloads its state. No editor is opened and no audio
//! device is used. Prints one line per plugin.
//!
//! ```text
//! PH_PLUGIN_SMOKE="C:\Program Files\Common Files\VST3\LABS.vst3" cargo test -p ph-plugin-host --test local_smoke -- --ignored --nocapture
//! ```

use ph_node::*;
use ph_plugin_host::*;
use std::sync::Arc;

struct NoAssets;
impl AssetResolver for NoAssets {
    fn sample(&self, r: &str) -> Result<Arc<SampleData>, String> {
        Err(format!("no {r}"))
    }
}

#[test]
#[ignore]
fn installed_plugins_load_process_and_keep_their_state() {
    let Ok(list) = std::env::var("PH_PLUGIN_SMOKE") else {
        eprintln!("set PH_PLUGIN_SMOKE to run");
        return;
    };
    let formats: Vec<Box<dyn PluginFormatHost>> = vec![
        Box::new(clap::ClapFormat::new()),
        Box::new(vst3::Vst3Format::new()),
    ];
    for path in list.split(';').filter(|p| !p.is_empty()) {
        let path = std::path::PathBuf::from(path);
        for f in &formats {
            let Ok(infos) = f.scan(&path) else { continue };
            for info in infos {
                let line = smoke(f.as_ref(), &info);
                println!("{}: {line}", info.name);
            }
        }
    }
}

fn smoke(f: &dyn PluginFormatHost, info: &PluginInfo) -> String {
    let inst = match f.instantiate(info) {
        Ok(i) => i,
        Err(e) => return format!("cannot instantiate: {e}"),
    };
    let control = inst.control();
    let mut node = PluginNode::new(inst);
    node.activate(&ActivateInfo {
        sample_rate: 48_000.0,
        max_frames: 256,
        quality: Quality::Live,
        seed: 1,
    });
    let n = 256;
    let mut peak = 0.0f32;
    let input = vec![0.1f32; n];
    for block in 0..375 {
        let mut events = Vec::new();
        if block % 50 == 0 {
            events.push(Event {
                frame: 0,
                kind: EventKind::NoteOn {
                    note_id: block,
                    key: 60,
                    velocity: 0.8,
                    pan: 0.0,
                    fine_pitch: 0.0,
                },
            });
        }
        if block % 50 == 25 {
            events.push(Event {
                frame: 0,
                kind: EventKind::NoteOff {
                    note_id: block - 25,
                    key: 60,
                    velocity: 0.0,
                },
            });
        }
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        let ins: [&[f32]; 2] = [&input, &input];
        let ports: [&[&[f32]]; 1] = [&ins];
        let mut ch: [&mut [f32]; 2] = [&mut l, &mut r];
        let mut outs: [&mut [&mut [f32]]; 1] = [&mut ch];
        let mut io = ProcessIo {
            inputs: if node.descriptor().audio_inputs.is_empty() {
                &[]
            } else {
                &ports
            },
            outputs: &mut outs,
            events: &events,
        };
        let ctx = ProcessContext {
            sample_rate: 48_000.0,
            frames: n,
            song_pos_samples: (block as usize * n) as i64,
            song_pos_ticks: 0.0,
            bpm: 120.0,
            playing: true,
            live: true,
        };
        node.process(&ctx, &mut io);
        peak = l.iter().chain(r.iter()).fold(peak, |m, v| m.max(v.abs()));
    }
    if let Some(fault) = node.fault_handle().fault() {
        return format!("CRASHED: {}", fault.message);
    }
    let params = node.descriptor().params.len();
    let state = node.save_state();
    let reload = node.load_state(&state, &NoAssets);
    format!(
        "ok: {params} params, latency {}, peak {peak:.3}, state {} bytes ({}), editor {}",
        control.as_ref().map_or(0, |c| c.latency()),
        state.len(),
        if reload.is_ok() {
            "reloads"
        } else {
            "reload failed"
        },
        info.has_editor,
    )
}
