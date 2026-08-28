//! Core types for the AI package.
//!
//! Message types, tool definitions, model metadata, usage tracking, and event
//! streams.

use std::{fmt, path::Path};

use serde::{Deserialize, Serialize};

// ── API & Provider Types ─────────────────────────────────────────────────────

/// Known APIs supported by providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Api {
    OpenaiCompletions,
}

/// Provider identifier.
pub type ProviderId = String;

/// Tool choice options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
}

// ── Content Types ────────────────────────────────────────────────────────────

/// Text content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    pub type_: String,
    pub text: String,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            type_: "text".to_string(),
            text: text.into(),
        }
    }
}

/// Thinking/reasoning content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingContent {
    pub type_: String,
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

impl ThinkingContent {
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            type_: "thinking".to_string(),
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: None,
        }
    }
}

/// Image content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContent {
    pub type_: String,
    pub data: String,
    pub mime_type: String,
}

impl ImageContent {
    pub fn new(
        data: impl Into<String>,
        mime_type: impl Into<String>,
    ) -> Self {
        Self {
            type_: "image".to_string(),
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

/// Tool call from an assistant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub type_: String,
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            type_: "toolCall".to_string(),
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: None,
            namespace: None,
        }
    }

    /// Return a human-readable representation of the tool call.
    ///
    /// Instead of showing raw JSON arguments, this formats the call based on
    /// the tool name so users can quickly understand what action was taken.
    pub fn human_readable(&self) -> String {
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        self.human_readable_with_cwd(&cwd)
    }

    /// Return a human-readable representation using `cwd` as the project
    /// directory for path shortening.
    pub fn human_readable_with_cwd(
        &self,
        cwd: &Path,
    ) -> String {
        let args = &self.arguments;

        match self.name.as_str() {
            "bash" => {
                if let Some(cmd) = args.get("command").and_then(|value| value.as_str()) {
                    format!("bash {cmd}")
                } else {
                    format!("{}", self.arguments)
                }
            },
            "read" | "write" | "edit" => {
                if let Some(path) = args.get("path").and_then(|value| value.as_str()) {
                    format!("{} {}", self.name, display_tool_path(path, cwd))
                } else {
                    format!("{}", self.arguments)
                }
            },
            "find" => {
                if let Some(pattern) = args.get("pattern").and_then(|value| value.as_str()) {
                    if let Some(path) = args.get("path").and_then(|value| value.as_str()) {
                        format!("find {} {}", pattern, display_tool_path(path, cwd))
                    } else {
                        format!("find {}", pattern)
                    }
                } else {
                    format!("{}", self.arguments)
                }
            },
            "grep" => {
                if let Some(pattern) = args.get("pattern").and_then(|value| value.as_str()) {
                    if let Some(path) = args.get("path").and_then(|value| value.as_str()) {
                        format!("grep {} {}", pattern, display_tool_path(path, cwd))
                    } else {
                        format!("grep {}", pattern)
                    }
                } else {
                    format!("{}", self.arguments)
                }
            },
            "ls" => {
                if let Some(path) = args.get("path").and_then(|value| value.as_str()) {
                    format!("ls {}", display_tool_path(path, cwd))
                } else {
                    "ls .".to_string()
                }
            },
            _ => format!("{}", self.arguments),
        }
    }
}

fn display_tool_path(
    path: &str,
    cwd: &Path,
) -> String {
    let displayed = crate::config::paths::display_path(path, cwd);
    if (path.ends_with('/') || path.ends_with('\\'))
        && displayed != "."
        && !displayed.ends_with('/')
        && !displayed.ends_with('\\')
    {
        format!("{displayed}{}", &path[path.len() - 1..])
    } else {
        displayed
    }
}

// ── Message Types ────────────────────────────────────────────────────────────

/// User message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
    /// Image attachments sent alongside the text content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
    pub timestamp: u64,
}

impl UserMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            images: Vec::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Create a user message with image attachments.
    pub fn with_images(
        content: impl Into<String>,
        images: Vec<ImageContent>,
    ) -> Self {
        let mut message = Self::new(content);
        message.images = images;
        message
    }
}

/// Assistant message with content, tool calls, and usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    pub api: Api,
    pub provider: ProviderId,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    pub timestamp: u64,
}

impl AssistantMessage {
    pub fn new(
        content: Vec<AssistantContent>,
        api: Api,
        provider: impl Into<String>,
        model: impl Into<String>,
        stop_reason: StopReason,
    ) -> Self {
        Self {
            content,
            api,
            provider: provider.into(),
            model: model.into(),
            response_model: None,
            response_id: None,
            usage: Usage::default(),
            stop_reason,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

/// Tool result message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<TextContent>,
    /// Image attachments returned by a tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    pub is_error: bool,
    pub timestamp: u64,
}

impl ToolResultMessage {
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: vec![TextContent::new(content)],
            images: Vec::new(),
            details: None,
            usage: None,
            added_tool_names: None,
            is_error,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

/// Unified message type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

// ── Assistant Content ────────────────────────────────────────────────────────

/// Content blocks in an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
}

// ── Stop Reason ──────────────────────────────────────────────────────────────

/// Reason the assistant stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
}

impl fmt::Display for StopReason {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        match self {
            StopReason::Pending => write!(f, "pending"),
            StopReason::Stop => write!(f, "stop"),
            StopReason::Length => write!(f, "length"),
            StopReason::ToolUse => write!(f, "tool_use"),
            StopReason::Error => write!(f, "error"),
            StopReason::Aborted => write!(f, "aborted"),
            StopReason::Deferred => write!(f, "deferred"),
        }
    }
}

// ── Usage ────────────────────────────────────────────────────────────────────

/// Token usage and cost tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: Cost,
}

/// Cost breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// Tool definition with JSON schema parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

// ── Model ────────────────────────────────────────────────────────────────────

/// Model cost rates per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCostRates {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// Model metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: ProviderId,
    pub base_url: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    pub cost: ModelCostRates,
    pub context_window: u32,
    pub max_tokens: u32,
}

impl Model {
    #[expect(
        clippy::too_many_arguments,
        reason = "model metadata is constructed from a fixed catalog schema"
    )]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        api: Api,
        provider: impl Into<String>,
        base_url: impl Into<String>,
        reasoning: bool,
        context_window: u32,
        max_tokens: u32,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            api,
            provider: provider.into(),
            base_url: base_url.into(),
            reasoning,
            input: vec!["text".to_string()],
            cost: ModelCostRates::default(),
            context_window,
            max_tokens,
        }
    }
}

// ── Context ──────────────────────────────────────────────────────────────────

/// Request context: system prompt, messages, and tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

impl Context {
    pub fn new(messages: impl IntoIterator<Item = Message>) -> Self {
        Self {
            messages: messages.into_iter().collect(),
            ..Default::default()
        }
    }

    pub fn with_system_prompt(
        mut self,
        prompt: impl Into<String>,
    ) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_tools(
        mut self,
        tools: Vec<Tool>,
    ) -> Self {
        self.tools = Some(tools);
        self
    }
}

// ── Streaming Options ────────────────────────────────────────────────────────

/// Options for streaming requests.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl StreamOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_api_key(
        mut self,
        key: impl Into<String>,
    ) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_temperature(
        mut self,
        temp: f64,
    ) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn with_max_tokens(
        mut self,
        max: u32,
    ) -> Self {
        self.max_tokens = Some(max);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_tool_shows_command_readable() {
        let call = ToolCall::new(
            "call-1",
            "bash",
            serde_json::json!({"command": "echo hello world"}),
        );
        assert_eq!(call.human_readable(), "bash echo hello world");
    }

    #[test]
    fn bash_tool_with_complex_command() {
        let call = ToolCall::new(
            "call-1",
            "bash",
            serde_json::json!({"command": "grep -r 'foo' src/ | wc -l"}),
        );
        assert_eq!(call.human_readable(), "bash grep -r 'foo' src/ | wc -l");
    }

    #[test]
    fn read_tool_shows_path() {
        let call = ToolCall::new(
            "call-1",
            "read",
            serde_json::json!({"path": "/path/to/file.rs"}),
        );
        assert_eq!(call.human_readable(), "read /path/to/file.rs");
    }

    #[test]
    fn write_tool_shows_path() {
        let call = ToolCall::new(
            "call-1",
            "write",
            serde_json::json!({"path": "/path/to/file.rs", "content": "hello"}),
        );
        assert_eq!(call.human_readable(), "write /path/to/file.rs");
    }

    #[test]
    fn tool_paths_are_relative_to_the_project_directory() {
        let project = std::path::PathBuf::from("/tmp/hallucinate-project");
        let call = ToolCall::new(
            "call-1",
            "read",
            serde_json::json!({"path": project.join("src/main.rs")}),
        );
        assert_eq!(call.human_readable_with_cwd(&project), "read src/main.rs");
    }

    #[test]
    fn edit_tool_shows_path() {
        let call = ToolCall::new(
            "call-1",
            "edit",
            serde_json::json!({"path": "/path/to/file.rs", "old_text": "foo", "new_text": "bar"}),
        );
        assert_eq!(call.human_readable(), "edit /path/to/file.rs");
    }

    #[test]
    fn find_tool_shows_pattern() {
        let call = ToolCall::new("call-1", "find", serde_json::json!({"pattern": "**/*.rs"}));
        assert_eq!(call.human_readable(), "find **/*.rs");
    }

    #[test]
    fn find_tool_with_path() {
        let call = ToolCall::new(
            "call-1",
            "find",
            serde_json::json!({"pattern": "*.toml", "path": "src"}),
        );
        assert_eq!(call.human_readable(), "find *.toml src");
    }

    #[test]
    fn grep_tool_shows_pattern() {
        let call = ToolCall::new("call-1", "grep", serde_json::json!({"pattern": "hello"}));
        assert_eq!(call.human_readable(), "grep hello");
    }

    #[test]
    fn grep_tool_with_path() {
        let call = ToolCall::new(
            "call-1",
            "grep",
            serde_json::json!({"pattern": "foo", "path": "src/"}),
        );
        assert_eq!(call.human_readable(), "grep foo src/");
    }

    #[test]
    fn ls_tool_shows_path() {
        let call = ToolCall::new("call-1", "ls", serde_json::json!({"path": "/some/dir"}));
        assert_eq!(call.human_readable(), "ls /some/dir");
    }

    #[test]
    fn ls_tool_defaults_to_current_dir() {
        let call = ToolCall::new("call-1", "ls", serde_json::json!({}));
        assert_eq!(call.human_readable(), "ls .");
    }

    #[test]
    fn unknown_tool_falls_back_to_json() {
        let call = ToolCall::new("call-1", "custom_tool", serde_json::json!({"foo": "bar"}));
        assert_eq!(call.human_readable(), r#"{"foo":"bar"}"#);
    }

    #[test]
    fn bash_tool_missing_command_falls_back() {
        let call = ToolCall::new("call-1", "bash", serde_json::json!({"timeout_ms": 5000}));
        assert_eq!(call.human_readable(), r#"{"timeout_ms":5000}"#);
    }
}
