//! Application state, event loop, rendering and input handling for the TAG UI.

pub(crate) mod actions;
pub(crate) mod event_loop;
pub(crate) mod events;
pub(crate) mod git_ui;
pub(crate) mod input;
pub(crate) mod panes;
pub(crate) mod paste;
pub(crate) mod render;
pub(crate) mod state;
pub(crate) mod tray;

pub(crate) use actions::*;
pub(crate) use event_loop::*;
pub(crate) use events::*;
pub(crate) use git_ui::*;
pub(crate) use input::*;
pub(crate) use panes::*;
pub(crate) use paste::*;
pub(crate) use render::*;
pub(crate) use state::*;
pub(crate) use tray::*;
