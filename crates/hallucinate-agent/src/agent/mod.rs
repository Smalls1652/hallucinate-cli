//! Agent core: multi-turn conversation with tool use.
//!
//! Provides the [`Agent`] struct with state management, tool registry,
//! context compaction, and system prompt assembly. The agent loop
//! coordinates turns with the LLM provider.

pub mod compaction;
pub mod prompt_templates;
pub mod runtime;
pub mod skills;
pub mod system_prompt;
pub mod tools;
pub mod types;
