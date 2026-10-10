//! Patternhaus desktop UI (DESIGN §9): egui panels over the `Studio` command interface.
//!
//! **Commands only.** Nothing here mutates a `Project`; every action is built in
//! [`mapping`] and submitted through `studio.session` as `Caller::human()`.

pub mod app;
pub mod browser_model;
pub mod core;
pub mod demo_agent;
pub mod dispatch;
pub mod keymap;
pub mod live;
pub mod mapping;
pub mod panels;
pub mod piano_roll;
pub mod playlist;
pub mod settings;
pub mod steps;
pub mod theme;
pub mod typing;
pub mod widgets;
pub mod windows;
pub mod workspace;

pub use app::PhApp;
pub use core::{AgentDirectory, AppCore, AppPaths, ClientInfo, GrantRequest, Hooks};
