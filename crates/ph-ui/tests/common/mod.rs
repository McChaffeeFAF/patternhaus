//! Shared helpers for the headless UI tests: a silent `Studio`, a built beat, and pumping.
#![allow(dead_code)]

use ph_commands::{Caller, Request, SessionHandle};
use ph_control::{AudioMode, Studio, StudioConfig};
use ph_core::{ChannelId, InstrumentSpec, PatternId, Project};
use ph_engine::EngineConfig;
use ph_ui::mapping::{self, NewNote};
use ph_ui::{AppPaths, Hooks, PhApp};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

pub struct Fixture {
    pub app: PhApp,
    pub ctx: egui::Context,
    pub dir: tempfile::TempDir,
}

pub fn studio(dir: &std::path::Path, project: Project) -> Studio {
    Studio::start(
        StudioConfig {
            project_dir: dir.to_path_buf(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            exports_dir: None,
            engine: EngineConfig::default(),
            audio: AudioMode::Silent,
        },
        project,
    )
    .expect("silent studio starts")
}

pub struct Ids {
    pub kick: ChannelId,
    pub snare: ChannelId,
    pub hat: ChannelId,
    pub bass: ChannelId,
    pub beat: PatternId,
    pub bassline: PatternId,
}

fn run(session: &SessionHandle, r: Request) -> ph_commands::Executed {
    session
        .execute(&Caller::human(), r.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", r.command))
}

fn sampler(name: &str) -> InstrumentSpec {
    InstrumentSpec::Sampler {
        sample: Some(format!("factory:drums/haus808/{name}")),
        root_key: 60,
        params: Default::default(),
    }
}

/// The M1 criterion-2 beat, built with the same requests the UI sends: 3 sampler channels
/// and a Haus Synth bass, step patterns, bass notes, EQ + compressor on the bass insert, a
/// reverb send, and an 8 bar arrangement.
pub fn build_beat(session: &SessionHandle) -> Ids {
    run(session, mapping::set_tempo(124.0));
    let beat = PatternId(
        run(session, mapping::pattern_add()).result["pattern"]
            .as_u64()
            .unwrap() as u32,
    );
    let bassline = PatternId(
        run(session, mapping::pattern_add()).result["pattern"]
            .as_u64()
            .unwrap() as u32,
    );
    let add = |name: &str, inst: InstrumentSpec, insert: u32| {
        ChannelId(
            run(session, mapping::channel_add(name, &inst, Some(insert))).result["channel"]
                .as_u64()
                .unwrap() as u32,
        )
    };
    let kick = add("Kick", sampler("kick"), 1);
    let snare = add("Snare", sampler("snare"), 2);
    let hat = add("Hat", sampler("hat_closed"), 3);
    let bass = add(
        "Bass",
        InstrumentSpec::Synth {
            preset: Some("bass/sub".into()),
            params: Default::default(),
        },
        4,
    );
    let row = |bits: &str| bits.chars().map(|c| c == 'x').collect::<Vec<_>>();
    run(
        session,
        mapping::step_set_row(beat, kick, &row("x...x...x...x...")),
    );
    run(
        session,
        mapping::step_set_row(beat, snare, &row("....x.......x...")),
    );
    run(
        session,
        mapping::step_set_row(beat, hat, &row("..x...x...x...x.")),
    );
    let notes: Vec<NewNote> = [(36u8, 0u64), (36, 960), (39, 1920), (34, 2880)]
        .iter()
        .map(|&(key, start)| NewNote {
            key,
            start,
            length: 720,
            velocity: 0.85,
            muted: false,
        })
        .collect();
    run(session, mapping::note_add(bassline, bass, &notes));
    // mix: EQ + compressor on the bass insert, reverb on a send insert
    run(session, mapping::effect_add(4, "eq"));
    run(session, mapping::effect_add(4, "compressor"));
    run(session, mapping::effect_add(5, "reverb"));
    run(session, mapping::send_add(2, 5, 0.4));
    run(session, mapping::send_add(3, 5, 0.25));
    // arrange 8 bars: drums every bar, bass from bar 3
    run(session, mapping::mixer_rename(5, "Reverb"));
    for bar in 0..8u64 {
        run(session, mapping::clip_add(0, beat, bar * 3840, None));
        if bar >= 2 {
            run(session, mapping::clip_add(1, bassline, bar * 3840, None));
        }
    }
    Ids {
        kick,
        snare,
        hat,
        bass,
        beat,
        bassline,
    }
}

pub fn fixture(beat: bool) -> (Fixture, Option<Ids>) {
    let dir = tempfile::tempdir().unwrap();
    let st = studio(dir.path(), Project::empty("Test project"));
    let ids = beat.then(|| build_beat(&st.session));
    let ctx = egui::Context::default();
    let app = PhApp::new(
        &ctx,
        st,
        AppPaths {
            project_dir: dir.path().to_path_buf(),
            assets_dir: assets_dir(),
            library_roots: vec![],
            config_dir: None,
        },
        Hooks::default(),
        EngineConfig::default(),
        None,
    )
    .expect("app starts");
    (Fixture { app, ctx, dir }, ids)
}

/// Pumps the core (commands replies, snapshots) for up to `ms`, until `done` is true.
pub fn pump_until(app: &mut PhApp, ms: u64, mut done: impl FnMut(&mut PhApp) -> bool) -> bool {
    let start = Instant::now();
    let mut t = 100.0;
    while start.elapsed() < Duration::from_millis(ms) {
        t += 0.01;
        app.core.update(t);
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    false
}

/// 100 channels, 50 patterns, a pattern with 10 000 notes and a long playlist.
pub fn big_project() -> Project {
    use ph_commands::{NullHost, Registry, Session};
    use serde_json::json;
    use std::sync::Arc;
    let mut s = Session::new(Arc::new(Registry::with_builtins()), Project::empty("Big"));
    let mut h = NullHost::default();
    let who = Caller::human();
    let mut run = |c: &str, p: serde_json::Value| {
        s.execute(&who, Request::new(c, p), &mut h).unwrap();
    };
    for _ in 0..50 {
        run("pattern.add", json!({}));
    }
    for i in 0..100 {
        let inst = if i % 4 == 3 {
            json!({"kind": "synth", "preset": "bass/sub"})
        } else {
            json!({"kind": "sampler", "sample": "factory:drums/haus808/kick", "root_key": 60})
        };
        run(
            "channel.add",
            json!({"name": format!("Channel {}", i + 1), "instrument": inst, "insert": 1 + (i % 32)}),
        );
    }
    let total = 64u64 * 3840;
    let notes: Vec<_> = (0..10_000u64)
        .map(|i| {
            json!({"key": 36 + ((i * 31) % 61) as u8,
                   "start": (i * 7919 * 15) % total / 120 * 120,
                   "length": 120 + (i % 4) * 120,
                   "velocity": 0.3 + (i % 7) as f32 * 0.1})
        })
        .collect();
    run("pattern.set_length", json!({"pattern": 0, "length": total}));
    run(
        "note.add",
        json!({"pattern": 0, "channel": 3, "notes": notes}),
    );
    for ch in 0..100u32 {
        let steps: Vec<bool> = (0..16).map(|s| (s + ch) % 4 == 0).collect();
        run(
            "step.set",
            json!({"pattern": 1, "channel": ch, "steps": steps}),
        );
    }
    for bar in 0..64u64 {
        run(
            "playlist.add_clip",
            json!({"track": bar % 8, "pattern": bar % 50, "start": bar * 3840}),
        );
    }
    s.project().as_ref().clone()
}

/// Waits until every command the UI sent has been answered and its snapshot arrived.
pub fn settle(app: &mut PhApp) {
    pump_until(app, 400, |a| a.core.dispatch.in_flight() == 0);
    // one more round for the Change stream
    pump_until(app, 60, |_| false);
}
