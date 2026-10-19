//! The automation-clip editor window (`WindowId::Automation`): glue between the editor widget
//! and the core.

use crate::auto_editor::{self, EditCtx};
use crate::core::AppCore;
use ph_core::ClipId;

pub fn show(ui: &mut egui::Ui, core: &mut AppCore, clip: ClipId) {
    let theme = core.theme.clone();
    let project = core.project.clone();
    let position = core.live().status.position;
    let mut st = core.auto_editors.remove(&clip).unwrap_or_default();
    let out = auto_editor::show(
        ui,
        &mut st,
        &EditCtx {
            project: &project,
            theme: &theme,
            clip,
            position,
        },
    );
    core.auto_editors.insert(clip, st);
    for o in out {
        match o {
            auto_editor::AeOut::Cmd(r) => core.send(r),
            auto_editor::AeOut::Batch(rs) => core.send_batch(rs),
        }
    }
}
