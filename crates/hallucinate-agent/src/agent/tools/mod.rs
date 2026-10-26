//! Tool system for the agent.
//!
//! Provides a trait-based tool system with built-in tools:
//! - `bash` - Shell command execution
//! - `read` - File reading with size limits
//! - `write` - Atomic file writing
//! - `edit` - Intelligent text editing with diff verification
//! - `find` - Glob-based file search
//! - `grep` - Pattern-based file content search
//! - `ls` - Directory listing

pub mod bash;
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
    Bash(BashToolConfig),
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
            BuiltInTool::Bash(config) => {
                let mut tool = bash::BashTool::new().with_shell(config.shell);

                if let Some(timeout) = config.timeout {
                    tool = tool.with_timeout(timeout);
                }

                Arc::new(tool)
            },

            BuiltInTool::Edit(_) => Arc::new(edit::EditTool::new()),

            BuiltInTool::Find(_) => Arc::new(find::FindTool::new()),

            BuiltInTool::Grep(_) => Arc::new(grep::GrepTool::new()),

            BuiltInTool::Ls(_) => Arc::new(ls::LsTool::new()),

            BuiltInTool::Read(config) => {
                Arc::new(read::ReadTool::new().with_auto_resize_images(config.auto_resize_images))
            },

            BuiltInTool::Write(_) => Arc::new(write::WriteTool::new()),
        }
    }
}

impl std::fmt::Display for BuiltInTool {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let value = match self {
            BuiltInTool::Bash(_) => "bash",
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

/// Configuration for the built-in bash tool.
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
pub struct BashToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
    /// The shell executable and arguments to use (e.g., `["/bin/bash", "-c"]`).
    pub shell: Option<Vec<String>>,
    /// Maximum execution timeout in seconds.
    pub timeout: Option<u64>,
}

impl Default for BashToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shell: None,
            timeout: None,
        }
    }
}

/// Configuration for the built-in edit tool.
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
pub struct EditToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
}

impl Default for EditToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Configuration for the built-in find tool.
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
pub struct FindToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
}

impl Default for FindToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Configuration for the built-in grep tool.
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
pub struct GrepToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
}

impl Default for GrepToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Configuration for the built-in ls tool.
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
pub struct LsToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
}

impl Default for LsToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Configuration for the built-in read tool.
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
pub struct ReadToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
    /// Whether to automatically resize images when reading files.
    pub auto_resize_images: bool,
}

impl Default for ReadToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_resize_images: true,
        }
    }
}

/// Configuration for the built-in write tool.
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
pub struct WriteToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
}

impl Default for WriteToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Return a list of all built-in tools with their default configurations.
#[cfg(test)]
pub fn all_built_in_tools_with_defaults() -> Vec<BuiltInTool> {
    vec![
        BuiltInTool::Bash(BashToolConfig::default()),
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
        assert!(registry.get_tool("bash").is_none());
    }

    #[test]
    fn test_registry_with_all_builtins() {
        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(all_built_in_tools_with_defaults());

        assert!(!registry.is_empty());
        assert!(registry.to_ai_tools().len() >= 7);
        assert!(registry.get_tool("bash").is_some());
        assert!(registry.get_tool("read").is_some());
        assert!(registry.get_tool("write").is_some());
    }

    #[test]
    fn test_registry_to_ai_tools() {
        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(all_built_in_tools_with_defaults());

        let ai_tools = registry.to_ai_tools();
        assert!(!ai_tools.is_empty());
        assert!(ai_tools.iter().any(|t| t.name == "bash"));
        assert!(ai_tools.iter().any(|t| t.name == "read"));
    }

    #[test]
    fn test_registry_with_nondefault_configs() {
        let tools = vec![
            BuiltInTool::Bash(BashToolConfig {
                enabled: true,
                shell: Some(vec!["/bin/bash".to_string(), "-c".to_string()]),
                timeout: Some(67),
            }),
            BuiltInTool::Read(ReadToolConfig {
                enabled: true,
                auto_resize_images: false,
            }),
        ];

        let mut registry = ToolRegistry::new();
        registry.add_builtin_tools(tools);

        assert!(registry.get_tool("bash").is_some());
        assert!(registry.get_tool("read").is_some());
    }
}
