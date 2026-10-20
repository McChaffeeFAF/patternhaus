//! The panels. Each exposes `show(ui, &mut AppCore)`; none touches a `Project` directly.

pub mod agents;
pub mod audio_editor;
pub mod automation;
pub mod browser;
pub mod history;
pub mod inspector;
pub mod mixer;
pub mod piano_roll;
pub mod playlist;
pub mod rack;
pub mod transport;

use crate::core::AppCore;
use crate::keymap::Context;

/// Marks the panel under the pointer as the keymap context. Call with the panel's rect.
pub fn track_hover(ui: &egui::Ui, core: &mut AppCore, ctx: Context) {
    if ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| ui.max_rect().contains(p))
    {
        core.hover_context = ctx;
    }
}
