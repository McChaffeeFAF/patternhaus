//! M2/T10 follow-ups seen through the app core, settling on the session snapshot.

mod common;

use common::*;
use ph_core::InstrumentSpec;

/// New channels and patterns made in the UI take the theme's colours, clear of the agent hue,
/// in both themes (the model's palette has an orchid next to Neon Slate's agent purple).
#[test]
fn new_channels_and_patterns_take_theme_colours() {
    for theme in ["neon-slate", "red-line"] {
        let (fx, _) = fixture(false);
        let mut app = fx.app;
        app.set_theme(theme);
        let allowed = app.core.theme.object_colors();
        let synth = InstrumentSpec::Synth {
            preset: None,
            params: Default::default(),
        };
        // one click at a time: each settles before the next
        for k in 0..6 {
            app.core.add_channel(&format!("Ch {k}"), &synth);
            app.core.add_pattern();
            settle(&mut app);
        }
        let p = app.core.studio.session.project().unwrap();
        assert_eq!(p.channels.len(), 6, "{theme}");
        assert_eq!(p.patterns.len(), 6, "{theme}");
        for (k, id) in p.channel_order.iter().enumerate() {
            let c = p.channels[id].color;
            assert_eq!(c, allowed[k % allowed.len()], "{theme}: channel {k}");
        }
        for (k, id) in p.pattern_order.iter().enumerate() {
            let c = p.patterns[id].color;
            assert_eq!(c, allowed[k % allowed.len()], "{theme}: pattern {k}");
        }
        // the UI's snapshot is the session's
        assert_eq!(*app.core.project, *p);
    }
}
