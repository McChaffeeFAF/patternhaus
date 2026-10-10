//! The control side of plan swapping, shared by the real-time engine and offline renders.

use crate::EngineResult;
use crate::plan::{CompileOptions, Plan, Settings, Shadow, compile};
use crate::rt::Mailbox;
use crate::stats::StatsBoard;
use ph_core::Project;
use ph_node::AssetResolver;
use std::sync::Arc;

/// Compiles snapshots against the plan the audio thread runs and posts them.
///
/// Protocol: the audio thread only ever *takes* from the mailbox. Before compiling, the
/// controller takes back a plan the audio thread has not picked up yet (and discards it):
/// then the plan the audio thread holds is exactly `installed`, so the new plan's
/// transfers refer to the right node indices. If the mailbox is empty, the pending plan
/// was taken, and it becomes `installed`.
pub(crate) struct Controller {
    pub settings: Settings,
    mailbox: Arc<Mailbox>,
    installed: Shadow,
    pending: Option<Shadow>,
    next_id: u64,
    pub opts: CompileOptions,
    pub board: Arc<StatsBoard>,
}

impl Controller {
    pub fn new(settings: Settings, mailbox: Arc<Mailbox>) -> Self {
        Self {
            settings,
            mailbox,
            installed: Shadow::empty(),
            pending: None,
            next_id: 1,
            opts: CompileOptions::default(),
            board: Arc::new(StatsBoard::default()),
        }
    }

    /// The first plan, handed straight to a new runtime.
    pub fn initial(
        &mut self,
        project: &Project,
        assets: &dyn AssetResolver,
    ) -> EngineResult<Box<Plan>> {
        let (plan, shadow) = compile(
            project,
            &Shadow::empty(),
            self.settings,
            &self.opts,
            assets,
            self.next_id,
        )?;
        self.next_id += 1;
        self.installed = shadow;
        self.pending = None;
        self.board = plan.board.clone();
        Ok(plan)
    }

    /// Compiles `project` and posts it for the audio thread's next block.
    pub fn submit(&mut self, project: &Project, assets: &dyn AssetResolver) -> EngineResult<()> {
        match self.mailbox.take() {
            Some(unclaimed) => {
                drop(unclaimed);
                self.pending = None;
            }
            None => {
                if let Some(s) = self.pending.take() {
                    self.installed = s;
                }
            }
        }
        let (plan, shadow) = compile(
            project,
            &self.installed,
            self.settings,
            &self.opts,
            assets,
            self.next_id,
        )?;
        self.next_id += 1;
        self.board = plan.board.clone();
        if let Some(stale) = self.mailbox.put(plan) {
            drop(stale);
        }
        self.pending = Some(shadow);
        Ok(())
    }
}
