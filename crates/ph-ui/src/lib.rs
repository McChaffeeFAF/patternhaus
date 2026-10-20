//! Patternhaus desktop UI (DESIGN §9): egui panels over the `Studio` command interface.
//!
//! **Commands only.** Nothing here mutates a `Project`; every action is built in
//! [`mapping`] and submitted through `studio.session` as `Caller::human()`.

pub mod about;
pub mod app;
pub mod audio_editor;
pub mod auto_editor;
pub mod automation;
pub mod brand;
pub mod browser_model;
pub mod core;
pub mod demo_agent;
pub mod dispatch;
pub mod dnd;
pub mod fonts;
pub mod hint;
pub mod icon;
pub mod icons;
pub mod keymap;
pub mod live;
pub mod mapping;
pub mod mapping_audio;
pub mod midi;
pub mod motion;
pub mod paint;
pub mod panels;
pub mod piano_roll;
pub mod playlist;
pub mod plugin_ui;
pub mod recorder;
pub mod settings;
pub mod splash;
pub mod steps;
pub mod theme;
pub mod toolbar;
pub mod typing;
pub mod waveform;
pub mod widgets;
pub mod windows;
pub mod workspace;

pub use app::PhApp;
pub use core::{AgentDirectory, AppCore, AppPaths, ClientInfo, GrantRequest, Hooks};
