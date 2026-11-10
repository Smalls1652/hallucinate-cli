//! Core types for the AI package.
//!
//! Message types, tool definitions, model metadata, usage tracking, and event
//! streams.

use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

// ── API & Provider Types ─────────────────────────────────────────────────────

/// Known APIs supported by providers.
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(
        Debug,
        Clone,
        Default,
        PartialEq,
        Eq,
        Serialize,
        Deserialize,
        schemars::JsonSchema
    )
)]
#[serde(rename_all = "kebab-case")]
pub enum Api {
    /// OpenAI Chat Completions-compatible API.
    #[default]
    OpenaiCompletions,

    /// OpenAI Responses-compatible API.
    OpenaiResponses,
}

/// Provider identifier.
pub type ProviderId = String;

/// Tool choice options.
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        Serialize,
        Deserialize,
        schemars::JsonSchema
    )
)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,

    None,
}

// ── Content Types ────────────────────────────────────────────────────────────

/// Text content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    /// Content type discriminator (always "text").
    pub type_: String,

    /// The text content.
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
    /// Content type discriminator (always "thinking").
    pub type_: String,

    /// The thinking/reasoning content text.
    pub thinking: String,

    /// Signature of the thinking content (for providers that use it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,

    /// Whether the thinking content was redacted.
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
    /// Content type discriminator (always "image").
    pub type_: String,

    /// Base64-encoded image data.
    pub data: String,

    /// MIME type of the image (e.g., "image/png").
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
    /// Content type discriminator (always "toolCall").
    pub type_: String,

    /// Unique identifier for this tool call.
    pub id: String,

    /// Name of the tool being called.
    pub name: String,

    /// JSON arguments passed to the tool.
    pub arguments: serde_json::Value,

    /// Signature of the thought that led to this tool call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,

    /// Namespace for the tool call.
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
}

// ── Message Types ────────────────────────────────────────────────────────────

/// User message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    /// The message content text.
    pub content: String,

    /// Image attachments sent alongside the text content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,

    /// Unix timestamp in milliseconds when the message was created.
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
    /// Content blocks in this message (text, thinking, or tool calls).
    pub content: Vec<AssistantContent>,

    /// The API used to generate this message.
    pub api: Api,

    /// Provider identifier that generated this message.
    pub provider: ProviderId,

    /// Model identifier used.
    pub model: String,

    /// Response model identifier from the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,

    /// Response identifier from the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,

    /// Token usage statistics.
    pub usage: Usage,

    /// Whether the provider included usage data in its response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_usage: bool,

    /// Reason the assistant stopped generating.
    pub stop_reason: StopReason,

    /// Error message if the response failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    /// Raw stop reason string from the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,

    /// Whether this turn ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,

    /// Unix timestamp in milliseconds when the message was created.
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
            has_usage: false,
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
    /// The tool call ID this result corresponds to.
    pub tool_call_id: String,

    /// Name of the tool that produced this result.
    pub tool_name: String,

    /// Text content of the result.
    pub content: Vec<TextContent>,

    /// Image attachments returned by a tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageContent>,

    /// Additional details about the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,

    /// Token usage from tool execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,

    /// Names of tools added during execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,

    /// Whether this result represents an error.
    pub is_error: bool,

    /// Unix timestamp in milliseconds when the result was created.
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
    /// A user message.
    User(UserMessage),

    /// An assistant message.
    Assistant(AssistantMessage),

    /// A tool result message.
    ToolResult(ToolResultMessage),
}

// ── Assistant Content ────────────────────────────────────────────────────────

/// Content blocks in an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    /// A text content block.
    Text(TextContent),

    /// A thinking/reasoning content block.
    Thinking(ThinkingContent),

    /// A tool call content block.
    ToolCall(ToolCall),
}

// ── Stop Reason ──────────────────────────────────────────────────────────────

/// Reason the assistant stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Generation completed normally.
    Pending,

    /// Generation stopped by the model.
    Stop,

    /// Generation stopped due to token limit.
    Length,

    /// Generation stopped because of tool use.
    ToolUse,

    /// Generation stopped due to an error.
    Error,

    /// Generation was aborted.
    Aborted,

    /// Generation was deferred.
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
    /// Number of input tokens consumed.
    pub input: u64,

    /// Number of output tokens generated.
    pub output: u64,

    /// Number of tokens read from cache.
    pub cache_read: u64,

    /// Number of tokens written to cache.
    pub cache_write: u64,

    /// Cache read tokens in the last hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,

    /// Number of reasoning tokens consumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,

    /// Total tokens consumed (input + output).
    pub total_tokens: u64,

    /// Cost breakdown for this usage.
    pub cost: Cost,
}

/// Cost breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    /// Cost for input tokens.
    pub input: f64,

    /// Cost for output tokens.
    pub output: f64,

    /// Cost for cache read tokens.
    pub cache_read: f64,

    /// Cost for cache write tokens.
    pub cache_write: f64,

    /// Total cost.
    pub total: f64,
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// Tool definition with JSON schema parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Unique name of the tool.
    pub name: String,

    /// Human-readable description of what the tool does.
    pub description: String,

    /// JSON schema defining the tool's parameters.
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
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, Default, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)
)]
pub struct ModelCostRates {
    /// Cost per million input tokens.
    #[serde(default)]
    pub input: f64,

    /// Cost per million output tokens.
    #[serde(default)]
    pub output: f64,

    /// Cost per million cache read tokens.
    #[serde(default)]
    pub cache_read: f64,

    /// Cost per million cache write tokens.
    #[serde(default)]
    pub cache_write: f64,
}

/// Amount of reasoning a model should perform for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,

    Low,

    Medium,

    High,

    Xhigh,

    Max,
}

impl ReasoningEffort {
    /// Return the default effort for models that support this request option.
    pub const fn default_for_supported_model() -> Self {
        Self::Medium
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let value = match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        };

        f.write_str(value)
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(
                "reasoning effort must be one of: none, low, medium, high, xhigh, max".to_string(),
            ),
        }
    }
}

/// Model metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Unique model identifier.
    pub id: String,

    /// Human-readable model name.
    pub name: String,

    /// API type used by this model.
    pub api: Api,

    /// Provider identifier for this model.
    pub provider: ProviderId,

    /// Base URL for the API endpoint.
    pub base_url: String,

    /// Whether this model supports reasoning/thinking output.
    pub reasoning: bool,

    /// Whether this model accepts the Responses API `reasoning.effort` option.
    pub supports_reasoning_effort: bool,

    /// Whether this model is hidden from the interactive model picker.
    pub hidden: bool,

    /// Supported input types (e.g., "text", "image").
    pub input: Vec<String>,

    /// Cost rates for this model.
    pub cost: ModelCostRates,

    /// Maximum context window size in tokens.
    pub context_window: u32,

    /// Maximum number of output tokens.
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
            supports_reasoning_effort: false,
            hidden: false,
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
    /// System prompt to prepend to messages.
    ///
    /// This is reference-counted because an agent reuses the same immutable
    /// prompt across request contexts in one conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<Arc<String>>,

    /// Conversation messages.
    pub messages: Vec<Message>,

    /// Available tools for the model.
    ///
    /// Tool schemas are immutable for a prepared request and are shared with
    /// the agent's cached context base until tool configuration changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Arc<Vec<Tool>>>,
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
        self.system_prompt = Some(Arc::new(prompt.into()));
        self
    }

    pub fn with_tools(
        mut self,
        tools: Vec<Tool>,
    ) -> Self {
        self.tools = Some(Arc::new(tools));
        self
    }
}

// ── Streaming Options ────────────────────────────────────────────────────────

/// A shared signal used to cancel an in-flight provider request.
///
/// Providers must observe this signal while establishing and consuming a
/// stream. Dropping the HTTP response body on cancellation closes the client
/// connection, which tells streaming providers to stop generating.
#[derive(Clone, Debug)]
pub struct RequestCancellation {
    cancelled: Arc<watch::Sender<bool>>,
}

impl Default for RequestCancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestCancellation {
    /// Create a cancellation signal that has not yet been cancelled.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(watch::channel(false).0),
        }
    }

    /// Request cancellation and wake all active waiters.
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    /// Return whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    /// Wait until cancellation is requested.
    pub async fn cancelled(&self) {
        let mut receiver = self.cancelled.subscribe();

        if !*receiver.borrow() {
            let _ = receiver.changed().await;
        }
    }
}

/// Options for streaming requests.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamOptions {
    /// API key override for this request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Maximum number of output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Tool choice behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Requested model reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// Session identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Include usage data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,

    /// Signal used to stop the HTTP request when the owning agent is
    /// cancelled. This is local runtime state and is never serialized.
    #[serde(skip)]
    pub cancellation: Option<RequestCancellation>,
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

    pub fn with_include_usage(
        mut self,
        include: bool,
    ) -> Self {
        self.include_usage = Some(include);
        self
    }

    /// Set the requested model reasoning effort.
    pub fn with_reasoning_effort(
        mut self,
        effort: ReasoningEffort,
    ) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// Attach a signal that cancels the in-flight provider request.
    pub fn with_cancellation(
        mut self,
        cancellation: RequestCancellation,
    ) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
}
