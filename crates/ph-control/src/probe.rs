//! `catalogue.probe` (M3/T9): plays probe notes through a plugin instrument restored from a
//! saved state, offline and headless (the editor is never opened), and measures them with
//! `ph_analysis::probe`.
//!
//! One render holds everything, so the plugin is loaded once (a sampler library may take a
//! while): the chromatic sweep, key by key at each velocity, then the test note on its own and
//! once after each candidate keyswitch. Each note is followed by a gap for its release; the
//! end of the gap is where the previous note's tail is measured.

use ph_analysis::probe::{Window, WindowLevel, differs, measure, sounding_keys};
use ph_commands::{CmdError, CmdResult, KeyswitchProbe, ProbeLevel, ProbeReport, ProbeRequest};
use ph_core::{Edit, InstrumentSpec, NoteSpec, Project};
use ph_engine::plugins::{PluginEnv, PluginState, PluginStatus};
use ph_engine::{PlayMode, RenderSettings};
use ph_node::{AssetResolver, Quality};
use std::collections::BTreeMap;

const STATE_REF: &str = "plugins/probe.state";
const BPM: f64 = 120.0;
const SAMPLE_RATE: u32 = 48_000;

fn host_err(e: impl std::fmt::Display) -> CmdError {
    CmdError::Host(e.to_string())
}

fn level(w: &WindowLevel) -> ProbeLevel {
    ProbeLevel {
        key: w.key,
        velocity: w.velocity,
        rms_db: w.rms_db,
        peak_db: w.peak_db,
        centroid_hz: w.centroid_hz,
        sounds: w.sounds,
    }
}

pub fn run(
    env: &PluginEnv,
    assets: &dyn AssetResolver,
    r: &ProbeRequest,
) -> CmdResult<ProbeReport> {
    let listed = env.host.find(r.format, &r.plugin_id).ok_or_else(|| {
        CmdError::Host(format!(
            "plugin {} has not been found by a plugin scan (run plugin.scan)",
            r.plugin_id
        ))
    })?;
    if listed.status != PluginStatus::Ok {
        return Err(CmdError::Host(format!(
            "{} cannot be loaded ({:?}: {})",
            listed.info.name, listed.status, listed.reason
        )));
    }
    let mut warnings = Vec::new();
    let mut states = BTreeMap::new();
    if !r.state.is_empty() {
        let st = PluginState::from_file_bytes(&r.state).map_err(host_err)?;
        if st.plugin_id != r.plugin_id || st.format != r.format {
            return Err(CmdError::Host(format!(
                "the saved state belongs to {}, not {}",
                st.plugin_id, r.plugin_id
            )));
        }
        if st.version != listed.info.version {
            warnings.push(format!(
                "the state was saved by version {} and is restored into {}",
                st.version, listed.info.version
            ));
        }
        states.insert(STATE_REF.to_string(), r.state.clone());
    }
    let env = env.with_states(states);

    let ticks_per_second = 960.0 * BPM / 60.0;
    let ticks = |s: f64| (s * ticks_per_second).round() as u64;
    let slot = r.note_seconds + r.gap_seconds;
    let tail = r.gap_seconds.min(0.1);

    let mut ed = Edit::new(&Project::empty("probe"));
    ed.update_settings(|s| {
        s.bpm = BPM;
        s.swing = 0.0;
    })
    .map_err(host_err)?;
    let ch = ed
        .add_channel(
            "probe",
            InstrumentSpec::Plugin {
                format: r.format,
                plugin_id: r.plugin_id.clone(),
                state: (!r.state.is_empty()).then(|| STATE_REF.to_string()),
                params: Default::default(),
            },
        )
        .map_err(host_err)?;
    let pat = ed.add_pattern("probe").map_err(host_err)?;
    let mut windows = Vec::new();
    let mut t = 0.0;
    let mut note = |ed: &mut Edit, key: u8, velocity: f32, windows: &mut Vec<Window>| {
        let spec = NoteSpec {
            velocity,
            ..NoteSpec::new(key, ticks(t), ticks(r.note_seconds).max(1))
        };
        ed.add_note(pat, ch, spec).map_err(host_err)?;
        windows.push(Window {
            key,
            velocity,
            start: t,
            end: t + r.note_seconds,
            tail: (t > 0.0 && tail > 0.0).then_some((t - tail, t)),
        });
        t += slot;
        Ok::<_, CmdError>(())
    };
    for v in &r.velocities {
        for k in &r.keys {
            note(&mut ed, *k, *v, &mut windows)?;
        }
    }
    let sweep_len = windows.len();
    const TEST_VELOCITY: f32 = 100.0 / 127.0;
    if !r.keyswitches.is_empty() {
        note(&mut ed, r.test_key, TEST_VELOCITY, &mut windows)?;
        for k in &r.keyswitches {
            note(&mut ed, *k, TEST_VELOCITY, &mut windows)?;
            note(&mut ed, r.test_key, TEST_VELOCITY, &mut windows)?;
        }
    }
    ed.add_clip(0, pat, 0).map_err(host_err)?;
    let (project, _) = ed.finish();

    let end = ticks(t);
    let settings = RenderSettings {
        mode: PlayMode::Song,
        range: Some((0, end)),
        sample_rate: SAMPLE_RATE,
        quality: Quality::Live,
        include_tail: false,
        block_frames: 512,
    };
    let audio = ph_engine::render_offline_with(&project, &settings, assets, Some(&env))
        .map_err(host_err)?;
    let levels = measure(&audio.channels, audio.sample_rate, &windows);
    let (sounding, range) = sounding_keys(&levels[..sweep_len]);

    let mut baseline = None;
    let mut keyswitches = Vec::new();
    if !r.keyswitches.is_empty() {
        let ks = &levels[sweep_len..];
        let mut prev = ks[0].clone();
        baseline = Some(level(&ks[0]));
        for pair in ks[1..].chunks(2) {
            let (cand, after) = (&pair[0], &pair[1]);
            keyswitches.push(KeyswitchProbe {
                key: cand.key,
                sounds: cand.sounds,
                after: level(after),
                changes_sound: differs(&prev, after),
            });
            prev = after.clone();
        }
    }
    Ok(ProbeReport {
        sweep: levels[..sweep_len].iter().map(level).collect(),
        sounding,
        range: range.map(|(a, b)| [a, b]),
        baseline,
        keyswitches,
        seconds: audio.frames() as f64 / audio.sample_rate as f64,
        warnings,
    })
}
