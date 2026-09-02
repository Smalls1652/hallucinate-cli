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
pub mod find;
pub mod grep;
pub mod ls;
mod paths;
pub mod read;
pub mod types;
pub mod write;

use std::sync::Arc;

pub use types::ToolResult;

use crate::{agent::tools::types::Tool, ai::types::Tool as AiTool};

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

    /// Create all built-in tools with the requested image processing policy.
    pub fn with_all_builtins_and_image_resize(auto_resize_images: bool) -> Self {
        let mut registry = Self::new();
        registry.add_tool(Arc::new(bash::BashTool::new()));
        registry.add_tool(Arc::new(
            read::ReadTool::new().with_auto_resize_images(auto_resize_images),
        ));
        registry.add_tool(Arc::new(write::WriteTool::new()));
        registry.add_tool(Arc::new(edit::EditTool::new()));
        registry.add_tool(Arc::new(find::FindTool::new()));
        registry.add_tool(Arc::new(grep::GrepTool::new()));
        registry.add_tool(Arc::new(ls::LsTool::new()));
        registry
    }

    /// Create all built-in tools with an optional configured shell executable.
    pub fn with_all_builtins_and_image_resize_and_shell(
        auto_resize_images: bool,
        shell: Option<&str>,
    ) -> Self {
        if shell.is_none() {
            return Self::with_all_builtins_and_image_resize(auto_resize_images);
        }

        let mut registry = Self::new();
        registry.add_tool(Arc::new(
            bash::BashTool::new().with_shell(shell.map(str::to_owned)),
        ));
        registry.add_tool(Arc::new(
            read::ReadTool::new().with_auto_resize_images(auto_resize_images),
        ));
        registry.add_tool(Arc::new(write::WriteTool::new()));
        registry.add_tool(Arc::new(edit::EditTool::new()));
        registry.add_tool(Arc::new(find::FindTool::new()));
        registry.add_tool(Arc::new(grep::GrepTool::new()));
        registry.add_tool(Arc::new(ls::LsTool::new()));
        registry
    }

    /// Add a tool to the registry.
    pub fn add_tool(
        &mut self,
        tool: Arc<dyn Tool + Send + Sync>,
    ) {
        self.tools.push(tool);
    }

    /// Get the list of available tools in AI format.
    pub fn to_ai_tools(&self) -> Vec<AiTool> {
        self.tools.iter().map(|t| t.to_ai_tool()).collect()
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
    ) -> Result<ToolResult, anyhow::Error> {
        match self.get_tool(tool_name) {
            Some(tool) => tool.execute(arguments, cwd).await,
            None => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
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
    fn test_registry_with_builtins() {
        let registry = ToolRegistry::with_all_builtins_and_image_resize(true);
        assert!(!registry.is_empty());
        assert!(registry.to_ai_tools().len() >= 7);
        assert!(registry.get_tool("bash").is_some());
        assert!(registry.get_tool("read").is_some());
        assert!(registry.get_tool("write").is_some());
    }

    #[test]
    fn test_registry_to_ai_tools() {
        let registry = ToolRegistry::with_all_builtins_and_image_resize(true);
        let ai_tools = registry.to_ai_tools();
        assert!(!ai_tools.is_empty());
        assert!(ai_tools.iter().any(|t| t.name == "bash"));
        assert!(ai_tools.iter().any(|t| t.name == "read"));
    }
}
