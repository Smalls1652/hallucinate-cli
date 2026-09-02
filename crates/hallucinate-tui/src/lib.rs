//! Terminal UI support for the interactive mode.
//!
//! The application intentionally uses one purpose-built event loop rather
//! than maintaining a second generic TUI framework.

pub mod alt_screen;
pub mod components;
pub mod input;
pub mod interactive;
pub mod keys;
pub mod slash_commands;
pub mod undo;
