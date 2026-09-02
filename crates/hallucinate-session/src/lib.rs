//! Session management for the interactive CLI.
//!
//! Provides session CRUD operations, JSONL-based persistence, and working
//! directory tracking.

mod cwd;
mod events;
pub mod manager;
pub mod storage;

pub use manager::{Session, SessionCreateOptions, SessionManager};
pub use storage::{SessionEntry, SessionInfo, SessionMetadata, SessionStorage};
