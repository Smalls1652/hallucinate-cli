//! Tool system for the agent.
//!
//! Provides a trait-based tool system with built-in tools:
//! - `shell` - Shell command execution
//! - `read` - File reading with size limits
//! - `write` - Atomic file writing
//! - `edit` - Intelligent text editing with diff verification
//! - `find` - Glob-based file search
//! - `grep` - Pattern-based file content search
//! - `ls` - Directory listing

pub mod edit;
pub mod error;
pub mod find;
pub mod grep;
pub mod limits;
pub mod ls;
pub mod mcp;
mod paths;
pub mod read;
pub mod script_tool;
pub mod shell;
mod shell_exec;
pub mod types;
mod walker;
pub mod write;

use std::sync::Arc;

pub use limits::ToolLimits;
pub use types::ToolResult;

use crate::{
    agent::tools::{
        error::InternalToolError,
        types::{Tool, ToolExecutionResult},
    },
    ai::types::Tool as AiTool,
    config::{
        EditToolConfig,
        FindToolConfig,
        GrepToolConfig,
        LsToolConfig,
        ReadToolConfig,
        ShellToolConfig,
        WriteToolConfig,
    },
};

/// Built-in tools available to the agent.
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(
        Debug,
        Clone,
        serde::Serialize,
        serde::Deserialize,
        schemars::JsonSchema
    )
)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltInTool {
    /// Shell command execution tool.
    Shell(ShellToolConfig),
    /// Intelligent text editing tool with diff verification.
    Edit(EditToolConfig),
    /// Glob-based file search tool.
    Find(FindToolConfig),
    /// Pattern-based file content search tool.
    Grep(GrepToolConfig),
    /// Directory listing tool.
    Ls(LsToolConfig),
    /// File reading tool with size limits and optional image resizing.
    Read(ReadToolConfig),
    /// Atomic file writing tool.
    Write(WriteToolConfig),
}

impl Into<Arc<dyn Tool + Send + Sync>> for BuiltInTool {
    fn into(self) -> Arc<dyn Tool + Send + Sync> {
        match self {
            BuiltInTool::Shell(config) => Arc::new(shell::ShellTool::new(Some(config))),

            BuiltInTool::Edit(_) => Arc::new(edit::EditTool::new()),

            BuiltInTool::Find(config) => Arc::new(find::FindTool::new(Some(config))),

            BuiltInTool::Grep(config) => Arc::new(grep::GrepTool::new(Some(config))),

            BuiltInTool::Ls(config) => Arc::new(ls::LsTool::new(Some(config))),

            BuiltInTool::Read(config) => Arc::new(read::ReadTool::new(Some(config))),

            BuiltInTool::Write(config) => Arc::new(write::WriteTool::new(Some(config))),
        }
    }
}

impl std::fmt::Display for BuiltInTool {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let value = match self {
            BuiltInTool::Shell(_) => "shell",
            BuiltInTool::Edit(_) => "edit",
            BuiltInTool::Find(_) => "find",
            BuiltInTool::Grep(_) => "grep",
            BuiltInTool::Ls(_) => "ls",
            BuiltInTool::Read(_) => "read",
            BuiltInTool::Write(_) => "write",
        };

        write!(f, "{value}")
    }
}

/// Return a list of all built-in tools with their default configurations.
#[cfg(test)]
pub fn all_built_in_tools_with_defaults() -> Vec<BuiltInTool> {
    vec![
        BuiltInTool::Shell(ShellToolConfig::default()),
        BuiltInTool::Edit(EditToolConfig::default()),
        BuiltInTool::Find(FindToolConfig::default()),
        BuiltInTool::Grep(GrepToolConfig::default()),
        BuiltInTool::Ls(LsToolConfig::default()),
        BuiltInTool::Read(ReadToolConfig::default()),
        BuiltInTool::Write(WriteToolConfig::default()),
    ]
}

/// Registry of available tools.
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool + Send + Sync>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Add all built-in tools to the registry.
    pub fn add_builtin_tools(
        &mut self,
        tools: Vec<BuiltInTool>,
    ) {
        for tool in tools {
            tracing::debug!("added built-in tool '{}' to registry", tool);
            self.tools.push(tool.into());
        }
    }

    /// Add an external tool to the registry.
    pub fn add_external_tool(
        &mut self,
        tool: Arc<dyn Tool + Send + Sync>,
    ) {
        tracing::debug!("added external tool '{}' to registry", tool.name());
        self.tools.push(tool);
    }

    pub fn remove_tool(
        &mut self,
        tool: &str,
    ) {
        let tool_index = match self.tools.iter().position(|item| item.name() == tool) {
            Some(value) => value,
            None => {
                tracing::warn!("No such tool named '{tool}'. Ignoring...");
                return;
            },
        };

        self.tools.remove(tool_index);
        tracing::debug!("Removed '{tool}' from tool registry.");
    }

    /// Get the list of available tools in AI format.
    pub fn to_ai_tools(&self) -> Vec<AiTool> {
        self.tools.iter().map(|t| t.to_ai_tool()).collect()
    }

    pub fn list_tools(&self) -> Vec<String> {
        self.to_ai_tools()
            .iter()
            .map(|item| item.name.clone())
            .collect()
    }

    /// Find a tool by name.
    pub fn get_tool(
        &self,
        name: &str,
    ) -> Option<&Arc<dyn Tool + Send + Sync>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    /// Check if the registry has any tools.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute a tool by name with the given arguments.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        cwd: Option<&str>,
    ) -> ToolExecutionResult {
        match self.get_tool(tool_name) {
            Some(tool) => tool.execute(arguments, cwd).await,
            None => Err(InternalToolError::UnknownTool(tool_name.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.get_tool("shell").is_none());
    }

    #[test]
    fn test_registry_with_all_builtins() {
        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(all_built_in_tools_with_defaults());

        assert!(!registry.is_empty());
        assert!(registry.to_ai_tools().len() >= 7);
        assert!(registry.get_tool("shell").is_some());
        assert!(registry.get_tool("read").is_some());
        assert!(registry.get_tool("write").is_some());
    }

    #[test]
    fn test_registry_to_ai_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(all_built_in_tools_with_defaults());

        let ai_tools = registry.to_ai_tools();
        assert!(!ai_tools.is_empty());
        assert!(ai_tools.iter().any(|t| t.name == "shell"));
        assert!(ai_tools.iter().any(|t| t.name == "read"));
    }

    #[test]
    fn test_registry_with_nondefault_configs() {
        let tools = vec![
            BuiltInTool::Shell(ShellToolConfig {
                shell: Some(vec!["/bin/bash".to_string(), "-c".to_string()]),
                timeout: 67,
                ..Default::default()
            }),
            BuiltInTool::Read(ReadToolConfig {
                auto_resize_images: false,
                ..Default::default()
            }),
        ];

        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(tools);

        assert!(registry.get_tool("shell").is_some());
        assert!(registry.get_tool("read").is_some());
    }
}
