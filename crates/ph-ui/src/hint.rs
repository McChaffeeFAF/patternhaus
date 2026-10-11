//! The hint bar's source and the right-click menu every control shares.
//!
//! Each widget registers a hint while the pointer is over it: its name, its current value and
//! its gestures. The bar at the bottom shows the hint of the previous frame, and the
//! application publishes it to the control interface so an agent can read it from
//! `get_state` (`view: "ui"`).

use egui::{Context, Id, Response, Ui};
use ph_control::UiHint;

const NEXT: &str = "ph_hint_next";
const NOW: &str = "ph_hint_now";
const HELP: &str = "ph_help_request";

/// Moves the hints registered during the last frame into place. Call once at the start of
/// every frame, before any widget is drawn.
pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|d| {
        let next: Option<UiHint> = d.remove_temp(Id::new(NEXT));
        if let Some(h) = next {
            d.insert_temp(Id::new(NOW), h);
        } else {
            d.remove::<UiHint>(Id::new(NOW));
        }
    });
}

/// The hint of whatever the pointer was over last frame.
pub fn current(ctx: &Context) -> Option<UiHint> {
    ctx.data(|d| d.get_temp::<UiHint>(Id::new(NOW)))
}

fn register(ctx: &Context, hint: UiHint) {
    ctx.data_mut(|d| d.insert_temp(Id::new(NEXT), hint));
}

/// A "Help" menu entry asks the shell to show this hint as a toast.
pub fn request_help(ctx: &Context, hint: UiHint) {
    ctx.data_mut(|d| d.insert_temp(Id::new(HELP), hint));
}

/// A request from a context menu's "Help" entry; the shell shows it as a toast.
pub fn take_help_request(ctx: &Context) -> Option<UiHint> {
    ctx.data_mut(|d| d.remove_temp::<UiHint>(Id::new(HELP)))
}

/// Hint registration for any widget response.
pub trait HintExt: Sized {
    /// Registers a hint: the control's name, its current value (may be empty) and its gestures.
    fn hint(self, name: &str, value: &str, gestures: &str) -> Self;
    /// A tooltip that is also the hint: `"Undo your last change (Ctrl+Z)"` becomes the name
    /// `Undo your last change` with the gesture `Ctrl+Z`.
    fn tip(self, text: &str) -> Self;
}

/// Splits `"Name (gestures)"`.
pub fn split_tip(text: &str) -> (String, String) {
    let t = text.trim();
    if t.ends_with(')')
        && let Some(i) = t.rfind(" (")
    {
        return (
            t[..i].trim().to_string(),
            t[i + 2..t.len() - 1].trim().to_string(),
        );
    }
    (t.to_string(), String::new())
}

impl HintExt for Response {
    fn hint(self, name: &str, value: &str, gestures: &str) -> Self {
        if self.hovered() || self.dragged() {
            register(
                &self.ctx,
                UiHint {
                    name: name.into(),
                    value: value.into(),
                    gestures: gestures.into(),
                },
            );
        }
        self
    }

    fn tip(self, text: &str) -> Self {
        let (name, gestures) = split_tip(text);
        self.on_hover_text(text).hint(&name, "", &gestures)
    }
}

/// What a context menu chose.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    None,
    Reset,
}

/// The entries every control's right-click menu ends with: Reset (when the control has a
/// default), Automate and MIDI learn (placeholders, disabled until they exist) and Help.
/// Call from inside a `context_menu` closure, after any entries of the control's own.
pub fn standard_entries(ui: &mut Ui, can_reset: bool, hint: &UiHint) -> MenuChoice {
    let mut choice = MenuChoice::None;
    if can_reset && ui.button(crate::theme::caps("Reset to default")).clicked() {
        choice = MenuChoice::Reset;
        ui.close();
    }
    ui.add_enabled(false, egui::Button::new(crate::theme::caps("Automate…")))
        .on_disabled_hover_text("Automation lands in a later milestone");
    ui.add_enabled(false, egui::Button::new(crate::theme::caps("MIDI learn…")))
        .on_disabled_hover_text("MIDI mapping lands in a later milestone");
    if ui.button(crate::theme::caps("Help")).clicked() {
        let h = hint.clone();
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(HELP), h));
        ui.close();
    }
    choice
}

/// The standard right-click menu on a response (no entries of its own).
pub fn standard_menu(resp: &Response, can_reset: bool, hint: &UiHint) -> MenuChoice {
    let mut choice = MenuChoice::None;
    resp.context_menu(|ui| {
        choice = standard_entries(ui, can_reset, hint);
    });
    choice
}

/// One line for the hint bar and for toasts: `name - value - gestures`.
pub fn line(h: &UiHint) -> String {
    let mut parts = vec![h.name.as_str()];
    if !h.value.is_empty() {
        parts.push(&h.value);
    }
    if !h.gestures.is_empty() {
        parts.push(&h.gestures);
    }
    parts.join("  \u{b7}  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltips_split_into_name_and_gestures() {
        assert_eq!(
            split_tip("Undo your last change (Ctrl+Z)"),
            ("Undo your last change".into(), "Ctrl+Z".into())
        );
        assert_eq!(split_tip("Space"), ("Space".into(), String::new()));
    }

    #[test]
    fn hints_move_into_place_one_frame_later() {
        let ctx = Context::default();
        register(
            &ctx,
            UiHint {
                name: "Tempo".into(),
                value: "124 BPM".into(),
                gestures: "drag".into(),
            },
        );
        assert!(current(&ctx).is_none());
        begin_frame(&ctx);
        assert_eq!(current(&ctx).unwrap().name, "Tempo");
        begin_frame(&ctx);
        assert!(current(&ctx).is_none(), "nothing hovered, nothing shown");
    }

    #[test]
    fn the_line_joins_what_is_known() {
        let h = UiHint {
            name: "Volume".into(),
            value: "-3.0 dB".into(),
            gestures: "drag, double-click resets".into(),
        };
        assert_eq!(
            line(&h),
            "Volume  \u{b7}  -3.0 dB  \u{b7}  drag, double-click resets"
        );
    }
}
