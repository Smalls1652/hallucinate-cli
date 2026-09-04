//! Tool trait and result types.

use std::pin::Pin;

use futures::Future;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    agent::tools::limits::{ToolLimits, truncate_utf8},
    ai::types::{ImageContent, Tool as AiTool},
};

/// A tool that can be called by the agent.
///
/// Tools are async functions that take JSON arguments and return a string
/// result.
pub trait Tool: Send + Sync + std::fmt::Debug {
    /// Returns the tool's unique name.
    fn name(&self) -> &str;

    /// Returns the tool's description (used for the AI tool definition).
    fn description(&self) -> &str;

    /// Returns the tool's JSON schema parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Convert this tool to an AI tool definition.
    fn to_ai_tool(&self) -> AiTool {
        AiTool::new(self.name(), self.description(), self.parameters())
    }

    /// Execute the tool with the given arguments.
    ///
    /// `arguments` should match the tool's JSON schema.
    /// `cwd` is the current working directory (optional).
    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>>;

    /// Execute the tool and optionally stream display output while it runs.
    ///
    /// The default preserves the behavior of tools that only produce a final
    /// result. Tools with incremental output, such as `bash`, override this.
    fn execute_with_output<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
        output: Option<ToolOutputSink>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>> {
        let _ = output;
        self.execute(arguments, cwd)
    }
}

/// Routes incremental output from a tool execution to the agent runtime.
#[derive(Debug, Clone)]
pub struct ToolOutputSink {
    sender: mpsc::UnboundedSender<ToolOutput>,
    tool_call_id: String,
    tool_name: String,
}

impl ToolOutputSink {
    /// Create an output sink for one provider-issued tool call.
    pub fn new(
        sender: mpsc::UnboundedSender<ToolOutput>,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        Self {
            sender,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
        }
    }

    /// Send output produced while the tool is still running.
    pub fn emit(
        &self,
        output: impl Into<String>,
    ) {
        let _ = self.sender.send(ToolOutput {
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            output: output.into(),
        });
    }
}

/// Incremental output emitted by a running tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Provider-issued tool call identifier.
    pub tool_call_id: String,
    /// Name of the emitting tool.
    pub tool_name: String,
    /// Newly produced display output.
    pub output: String,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool call ID this result corresponds to.
    pub tool_call_id: String,
    /// The name of the tool that was executed.
    pub tool_name: String,
    /// The result content (success or error message).
    pub content: String,
    /// Image attachments returned by the tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
    /// Whether this result represents an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a new successful tool result.
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: truncate_utf8(
                &content.into(),
                ToolLimits::default().max_tool_result_bytes,
                "Tool result",
            ),
            images: Vec::new(),
            is_error: false,
        }
    }

    /// Create a new error tool result.
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: truncate_utf8(
                &error.into(),
                ToolLimits::default().max_tool_result_bytes,
                "Tool result",
            ),
            images: Vec::new(),
            is_error: true,
        }
    }

    /// Attach images to a successful tool result.
    pub fn with_images(
        mut self,
        images: Vec<ImageContent>,
    ) -> Self {
        self.images = images;
        self
    }
}

/// Convert a tool result to a tool result message for the AI context.
pub fn tool_result_to_message(result: &ToolResult) -> crate::ai::types::ToolResultMessage {
    let mut message = crate::ai::types::ToolResultMessage::new(
        &result.tool_call_id,
        &result.tool_name,
        &result.content,
        result.is_error,
    );

    if result.is_error {
        message.content.push(crate::ai::types::TextContent::new(
            "\n[This is an error result. The tool execution failed.]",
        ));
    }

    message.images = result.images.clone();

    message
}

/// Convert a tool result to a tool result message for the AI context.
///
/// This is a convenience function that calls [`tool_result_to_message`].
pub fn tool_result_to_message_for_context(result: &ToolResult) -> crate::ai::types::Message {
    crate::ai::types::Message::ToolResult(tool_result_to_message(result))
}
