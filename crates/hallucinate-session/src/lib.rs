//! Session management for the interactive CLI.
//!
//! Provides session CRUD operations, JSONL-based persistence, and working
//! directory tracking.

pub mod commands;
pub mod cwd;
pub mod manager;
pub mod storage;

pub use commands::{
    BUILTIN_COMMANDS, BUILTIN_INVOCATIONS, CommandInfo, SlashCommand, parse,
    resolve_builtin_command,
};
pub use cwd::SessionCwd;
pub use manager::{Session, SessionCreateOptions, SessionManager};
pub use storage::{SessionEntry, SessionInfo, SessionMetadata, SessionStorage};
