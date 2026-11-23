//! Application state, event loop, rendering and input handling for the TAG UI.

pub(crate) mod actions;
pub(crate) mod control;
pub(crate) mod control_act;
pub(crate) mod control_danger;
pub(crate) mod control_runs;
pub(crate) mod event_loop;
pub(crate) mod events;
pub(crate) mod git_ui;
pub(crate) mod input;
pub(crate) mod panes;
pub(crate) mod paste;
pub(crate) mod paths;
pub(crate) mod render;
pub(crate) mod sessions;
pub(crate) mod state;
pub(crate) mod tray;

pub(crate) use actions::*;
pub(crate) use control::*;
pub(crate) use control_act::*;
pub(crate) use control_danger::*;
pub(crate) use control_runs::*;
pub(crate) use event_loop::*;
pub(crate) use events::*;
pub(crate) use git_ui::*;
pub(crate) use input::*;
pub(crate) use panes::*;
pub(crate) use paste::*;
pub(crate) use paths::*;
pub(crate) use render::*;
pub(crate) use sessions::*;
pub(crate) use state::*;
pub(crate) use tray::*;
