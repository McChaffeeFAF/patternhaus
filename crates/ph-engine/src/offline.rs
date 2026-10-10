//! Offline rendering: the same [`Runtime`] driven by a fake clock.
//!
//! [`Offline`] is a scripted engine for tests and exports: post projects and commands
//! between blocks exactly as the control thread would, and pull blocks of any size. Plans
//! and commands go through the same mailbox and command ring as the real-time engine, so
//! they take effect at the same block boundary.

use crate::control::Controller;
use crate::plan::Settings;
use crate::rt::{Mailbox, Retire, enable_ftz_daz, restore_fp};
use crate::runtime::{Command, Runtime};
use crate::stats::Shared;
use crate::{AudioBuffer, EngineResult, PlayMode, RenderSettings, TransportCommand};
use ph_core::{MIXER_INSERTS, Project};
use ph_node::AssetResolver;
use std::sync::Arc;

/// −96 dBFS.
const TAIL_THRESHOLD: f32 = 1.584_893_2e-5;
/// Tails stop after this long below the threshold…
const TAIL_QUIET_SECONDS: f64 = 1.0;
/// …or after this long in total.
const TAIL_MAX_SECONDS: f64 = 10.0;
/// Tail detection works on windows of this many frames (independent of block size).
const TAIL_WINDOW: usize = 1024;

#[cfg_attr(not(test), allow(dead_code))] // scripting is used by tests (and the T7 harness)
pub(crate) struct Offline {
    pub rt: Runtime,
    ctl: Controller,
    cmds: rtrb::Producer<Command>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Offline {
    /// An engine with `project` loaded, stopped at 0. `workers` = extra graph threads.
    pub fn new(
        project: &Project,
        settings: Settings,
        assets: &dyn AssetResolver,
        workers: usize,
    ) -> EngineResult<Self> {
        let mailbox = Arc::new(Mailbox::new());
        let mut ctl = Controller::new(settings, mailbox.clone());
        let plan = ctl.initial(project, assets)?;
        let (tx, rx) = rtrb::RingBuffer::new(1024);
        let rt = Runtime::new(
            plan,
            mailbox,
            Retire::Drop,
            rx,
            Arc::new(Shared::new(MIXER_INSERTS + 1)),
            workers,
            1,
        );
        Ok(Self { rt, ctl, cmds: tx })
    }

    /// Posts a new snapshot; it is installed at the start of the next block.
    pub fn set_project(
        &mut self,
        project: &Project,
        assets: &dyn AssetResolver,
    ) -> EngineResult<()> {
        self.ctl.submit(project, assets)
    }

    /// Queues a command for the next block.
    pub fn command(&mut self, cmd: Command) {
        let _ = self.cmds.push(cmd);
    }

    pub fn transport(&mut self, t: TransportCommand) {
        self.command(Command::Transport(t));
    }

    /// Renders `frames` frames in blocks of `block` (FTZ/DAZ on, like the audio threads).
    pub fn render(&mut self, frames: usize, block: usize) -> [Vec<f32>; 2] {
        let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
        self.render_into(&mut out, 0, frames, block);
        out
    }

    fn render_into(&mut self, out: &mut [Vec<f32>; 2], from: usize, frames: usize, block: usize) {
        let saved = enable_ftz_daz();
        let block = block.max(1);
        let mut done = 0;
        let [l, r] = out;
        while done < frames {
            let n = block.min(frames - done);
            let a = from + done;
            self.rt.process(&mut l[a..a + n], &mut r[a..a + n]);
            done += n;
        }
        restore_fp(saved);
    }
}

/// See [`crate::render_offline`].
pub(crate) fn render(
    project: &Project,
    settings: &RenderSettings,
    assets: &dyn AssetResolver,
    workers: usize,
) -> EngineResult<AudioBuffer> {
    let block = settings.block_frames.clamp(1, 16_384);
    let s = Settings {
        sample_rate: settings.sample_rate.max(1),
        max_frames: block,
        quality: settings.quality,
    };
    let mut off = Offline::new(project, s, assets, workers)?;
    let bar = project.settings.time_signature.ticks_per_bar();
    let (a, b) = settings.range.unwrap_or(match settings.mode {
        PlayMode::Song => (0, project.song_length()),
        PlayMode::Pattern(id) => (0, project.pattern_length(id).unwrap_or(bar)),
    });
    let b = b.max(a);
    let tempo = &off.rt.plan.tempo;
    let (start, end) = (tempo.sample_at(a), tempo.sample_at(b));
    off.rt
        .apply(Command::Transport(TransportCommand::SetMode(settings.mode)));
    off.rt.apply(Command::Transport(TransportCommand::Seek(a)));
    off.rt.apply(Command::Transport(TransportCommand::Play));
    off.rt.tr.looping = false;
    off.rt.tr.end = Some(end);
    let frames = (end - start) as usize;
    let mut out = off.render(frames, block);
    if settings.include_tail {
        let sr = s.sample_rate as f64;
        let max_tail = (TAIL_MAX_SECONDS * sr) as usize;
        let quiet_needed = (TAIL_QUIET_SECONDS * sr) as usize;
        let mut tail = [Vec::with_capacity(max_tail), Vec::with_capacity(max_tail)];
        let mut keep = 0usize;
        let mut rendered = 0usize;
        while rendered < max_tail {
            let w = TAIL_WINDOW.min(max_tail - rendered);
            tail[0].resize(rendered + w, 0.0);
            tail[1].resize(rendered + w, 0.0);
            off.render_into(&mut tail, rendered, w, block);
            let (l, r) = (&tail[0][rendered..], &tail[1][rendered..]);
            for (i, (a, b)) in l.iter().zip(r).enumerate() {
                if a.abs() >= TAIL_THRESHOLD || b.abs() >= TAIL_THRESHOLD {
                    keep = rendered + i + 1;
                }
            }
            rendered += w;
            if rendered - keep >= quiet_needed {
                break;
            }
        }
        for c in 0..2 {
            out[c].extend_from_slice(&tail[c][..keep]);
        }
    }
    let [l, r] = out;
    Ok(AudioBuffer {
        sample_rate: s.sample_rate,
        channels: vec![l, r],
    })
}
