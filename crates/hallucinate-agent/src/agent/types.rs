//! Agent-specific types: states, events, and configuration.

use std::{fmt, sync::LazyLock};

use base64::Engine;
use image::GenericImageView;
use tokenizers::{Tokenizer, models::wordlevel::WordLevel, pre_tokenizers::whitespace::Whitespace};

use crate::ai::types::{
    AssistantMessage,
    Context,
    ImageContent,
    Message,
    Model,
    RequestCancellation,
    StopReason,
    Tool,
    ToolCall,
};

// ── Agent State ──────────────────────────────────────────────────────────────

/// Handle used by callers that do not own the running agent to request
/// cancellation (for example, the interactive UI while a task owns `Agent`).
#[derive(Clone, Debug)]
pub struct AgentCancellation {
    /// Shared provider-level cancellation signal.
    signal: RequestCancellation,
}

impl AgentCancellation {
    /// Create a new `AgentCancellation` signal.
    pub(crate) fn new() -> Self {
        Self {
            signal: RequestCancellation::new(),
        }
    }

    /// Request cancellation of the active generation.
    pub fn cancel(&self) {
        self.signal.cancel();
    }

    /// Return whether cancellation has been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.signal.is_cancelled()
    }

    /// Create a fresh `AgentCancellation` with a new notification handle.
    pub(crate) fn fresh() -> Self {
        Self::new()
    }

    /// Return the provider cancellation signal for the active request.
    pub(crate) fn request_cancellation(&self) -> RequestCancellation {
        self.signal.clone()
    }

    /// Wait until cancellation has been requested.
    ///
    /// Returns immediately if already cancelled.
    pub(crate) async fn cancelled(&self) {
        self.signal.cancelled().await;
    }
}

/// Current state of the agent's lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Agent is idle, waiting for input.
    Idle,

    /// Agent is streaming a response from the LLM.
    Streaming,

    /// Agent is executing tool calls.
    ToolExecuting,

    /// Agent has completed a full turn.
    TurnComplete,

    /// Agent has encountered an error.
    Error(String),

    /// Agent has been aborted.
    Aborted,
}

impl fmt::Display for AgentState {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        match self {
            AgentState::Idle => write!(f, "idle"),
            AgentState::Streaming => write!(f, "streaming"),
            AgentState::ToolExecuting => write!(f, "tool_executing"),
            AgentState::TurnComplete => write!(f, "turn_complete"),
            AgentState::Error(msg) => write!(f, "error: {msg}"),
            AgentState::Aborted => write!(f, "aborted"),
        }
    }
}

// ── Agent Events ─────────────────────────────────────────────────────────────

/// Events emitted by the agent during operation.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent state changed.
    StateChanged {
        /// New agent state.
        new_state: AgentState,

        /// Previous agent state.
        old_state: AgentState,
    },

    /// A new turn started.
    TurnStarted { turn_number: u32 },

    /// A text fragment from the assistant response.
    AssistantTextDelta { delta: String },

    /// A reasoning fragment from the assistant response.
    AssistantThinkingDelta { delta: String },

    /// A complete assistant response for a turn.
    AssistantMessageCompleted {
        /// Turn number at completion.
        turn_number: u32,

        /// The completed assistant message.
        message: Box<AssistantMessage>,
    },

    /// The estimated number of tokens currently retained in the context.
    TokenCountUpdated { token_count: u64 },

    /// A streamed tool call began. Its arguments may still be incomplete.
    ToolCallStarted {
        /// Content index of the tool call.
        content_index: usize,

        /// The tool call that started.
        tool_call: ToolCall,
    },

    /// A streamed tool call received updated metadata or arguments.
    ///
    /// The call contains all argument text accumulated so far as a raw JSON
    /// string until the provider completes the call.
    ToolCallDelta {
        /// Content index of the tool call.
        content_index: usize,

        /// The updated tool call.
        tool_call: ToolCall,
    },

    /// Tool call detected in assistant response.
    ToolCallDetected {
        /// Unique identifier for the tool call.
        tool_call_id: String,

        /// Name of the tool to execute.
        tool_name: String,

        /// JSON arguments passed to the tool.
        arguments: serde_json::Value,
    },

    /// Tool execution started.
    ToolExecutionStarted {
        /// Unique identifier for the tool call.
        tool_call_id: String,

        /// Name of the tool to execute.
        tool_name: String,
    },

    /// Incremental output from a tool that is still executing.
    ToolExecutionOutput {
        /// Unique identifier for the tool call.
        tool_call_id: String,

        /// Name of the tool to execute.
        tool_name: String,

        /// Output text produced by the tool.
        output: String,
    },

    /// Tool execution completed.
    ToolExecutionCompleted {
        /// Unique identifier for the tool call.
        tool_call_id: String,

        /// Name of the tool to execute.
        tool_name: String,

        /// Output text produced by the tool.
        result: String,

        /// Whether the result indicates an error.
        is_error: bool,
    },

    /// Tool execution failed.
    ToolExecutionFailed {
        /// Unique identifier for the tool call.
        tool_call_id: String,

        /// Name of the tool to execute.
        tool_name: String,

        /// Error message describing the failure.
        error: String,
    },

    /// Turn completed successfully.
    TurnCompleted {
        /// Turn number at completion.
        turn_number: u32,

        /// Reason why the assistant stopped.
        stop_reason: StopReason,
    },

    /// Error occurred during agent operation.
    Error { message: String },

    /// Context compaction performed.
    CompactionPerformed {
        /// Token count before compaction.
        tokens_before: u64,

        /// Token count after compaction.
        tokens_after: u64,
        
        /// Number of summaries added during compaction.
        summaries_added: u32,
    },

    /// Agent aborted.
    Aborted,
}

impl fmt::Display for AgentEvent {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        match self {
            AgentEvent::StateChanged { new_state, .. } => {
                write!(f, "[state] {new_state}")
            },

            AgentEvent::TurnStarted { turn_number } => {
                write!(f, "[turn] started #{turn_number}")
            },

            AgentEvent::AssistantTextDelta { delta, .. } => {
                write!(f, "[text_delta len={}]", delta.len())
            },

            AgentEvent::AssistantThinkingDelta { delta } => {
                write!(f, "[thinking_delta len={}]", delta.len())
            },

            AgentEvent::AssistantMessageCompleted { turn_number, .. } => {
                write!(f, "[assistant] completed turn #{turn_number}")
            },

            AgentEvent::TokenCountUpdated { token_count } => {
                write!(f, "[token_count] {token_count}")
            },

            AgentEvent::ToolCallStarted { tool_call, .. } => {
                write!(f, "[tool_call] writing {}", tool_call.name)
            },

            AgentEvent::ToolCallDelta { tool_call, .. } => {
                write!(f, "[tool_call] updating {}", tool_call.name)
            },

            AgentEvent::ToolCallDetected { tool_name, .. } => {
                write!(f, "[tool_call] {tool_name}")
            },

            AgentEvent::ToolExecutionStarted { tool_name, .. } => {
                write!(f, "[tool_exec] starting {tool_name}")
            },

            AgentEvent::ToolExecutionOutput { tool_name, .. } => {
                write!(f, "[tool_output] {tool_name}")
            },

            AgentEvent::ToolExecutionCompleted { tool_name, .. } => {
                write!(f, "[tool_done] {tool_name}")
            },

            AgentEvent::ToolExecutionFailed { tool_name, .. } => {
                write!(f, "[tool_failed] {tool_name}")
            },

            AgentEvent::TurnCompleted { turn_number, .. } => {
                write!(f, "[turn] completed #{turn_number}")
            },

            AgentEvent::Error { message } => {
                write!(f, "[error] {message}")
            },

            AgentEvent::CompactionPerformed {
                tokens_before,
                tokens_after,
                ..
            } => {
                write!(f, "[compaction] {tokens_before} -> {tokens_after} tokens")
            },

            AgentEvent::Aborted => write!(f, "[abort]"),
        }
    }
}

// ── Agent Configuration ──────────────────────────────────────────────────────

/// Configuration for the agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Maximum number of turns before stopping.
    pub max_turns: u32,

    /// Maximum context window size in tokens.
    pub max_context_tokens: u32,

    /// Whether to enable automatic context compaction.
    pub enable_compaction: bool,

    /// Whether to enable tool use.
    pub enable_tools: bool,

    /// Default temperature for LLM calls.
    pub temperature: Option<f64>,

    /// Default max tokens for LLM calls.
    pub max_tokens: Option<u32>,

    /// Whether streaming requests include usage data.
    pub include_usage: Option<bool>,

    /// Whether to include tool definitions in context.
    pub include_tool_definitions: bool,

    /// Maximum number of independent tool calls to execute at once.
    pub max_concurrent_tool_calls: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 120,
            max_context_tokens: 128_000,
            enable_compaction: true,
            enable_tools: true,
            temperature: None,
            max_tokens: None,
            include_usage: None,
            include_tool_definitions: true,
            max_concurrent_tool_calls: 4,
        }
    }
}

// ── Turn Context ─────────────────────────────────────────────────────────────

/// Information tracked during a single turn.
#[derive(Debug, Clone)]
pub struct TurnContext {
    /// Tool calls pending execution.
    pub pending_tool_calls: Vec<PendingToolCall>,

    /// Tool results collected so far.
    pub tool_results: Vec<crate::agent::tools::ToolResult>,
}

impl TurnContext {
    /// Create a new `TurnContext` with empty pending tool calls and results.
    pub fn new(_turn_number: u32) -> Self {
        Self {
            pending_tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    /// Add a tool call to the pending list for later execution.
    pub fn add_pending_tool_call(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    ) {
        self.pending_tool_calls.push(PendingToolCall {
            tool_call_id,
            tool_name,
            arguments,
        });
    }

    /// Remove a completed tool call from the pending list by its ID.
    pub fn remove_completed_tool_call(
        &mut self,
        tool_call_id: &str,
    ) {
        self.pending_tool_calls
            .retain(|tc| tc.tool_call_id != tool_call_id);
    }
}

/// A tool call that is pending execution.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    /// Unique identifier for the tool call.
    pub tool_call_id: String,

    /// Name of the tool to execute.
    pub tool_name: String,

    /// JSON arguments passed to the tool.
    pub arguments: serde_json::Value,
}

// ── Message History ──────────────────────────────────────────────────────────

/// Provider-neutral token estimator used for context pressure decisions.
///
/// There is no dependable universal tokenizer for the OpenAI-compatible and
/// custom models supported by this crate. This estimator uses `tokenizers` to
/// split text into words and punctuation, then estimates each piece's subword
/// cost conservatively. This is more representative than counting raw bytes
/// while remaining independent of a particular provider's tokenizer.
#[derive(Debug, Default)]
pub struct TokenEstimator;

/// Tokenize lexical pieces consistently before estimating their subword cost.
///
/// The word-level model intentionally maps every piece to its unknown token:
/// its vocabulary is not used for counting, while its pre-tokenizer supplies
/// the stable word-and-punctuation boundaries used by the estimate.
static ESTIMATION_TOKENIZER: LazyLock<Tokenizer> = LazyLock::new(|| {
    let mut tokenizer = Tokenizer::new(WordLevel::default());
    tokenizer.with_pre_tokenizer(Some(Whitespace));
    tokenizer
});

impl TokenEstimator {
    /// Create an estimator for `model`.
    ///
    /// The model is accepted to keep cache invalidation explicit at model
    /// switches, even though the provider-neutral fallback is currently used
    /// for all model families.
    pub fn for_model(_model: &Model) -> Self {
        Self
    }

    /// Return the conservative estimator for callers without model metadata,
    /// including compatibility helpers and generic compaction.
    pub fn fallback() -> Self {
        Self
    }

    /// Estimate the number of tokens in one message, including wire framing.
    pub fn estimate_message_tokens(
        &self,
        message: &Message,
    ) -> u64 {
        const MESSAGE_FRAMING_TOKENS: u64 = 4;

        let content_tokens = match message {
            Message::User(message) => {
                self.estimate_text_tokens(&message.content)
                    + message
                        .images
                        .iter()
                        .map(estimate_image_tokens)
                        .sum::<u64>()
            },

            Message::Assistant(message) => message
                .content
                .iter()
                .map(|content| match content {
                    crate::ai::types::AssistantContent::Text(text) => {
                        self.estimate_text_tokens(&text.text)
                    },

                    crate::ai::types::AssistantContent::Thinking(thinking) => {
                        self.estimate_text_tokens(&thinking.thinking)
                    },

                    crate::ai::types::AssistantContent::ToolCall(call) => {
                        10 + self.estimate_text_tokens(&call.name)
                            + self.estimate_json_tokens(&call.arguments)
                    },
                })
                .sum(),

            Message::ToolResult(message) => {
                message
                    .content
                    .iter()
                    .map(|content| self.estimate_text_tokens(&content.text))
                    .sum::<u64>()
                    + message
                        .images
                        .iter()
                        .map(estimate_image_tokens)
                        .sum::<u64>()
            },
        };

        MESSAGE_FRAMING_TOKENS + content_tokens
    }

    /// Estimate system-prompt and tool-definition tokens which do not vary
    /// with conversation history.
    pub fn estimate_static_context_tokens(
        &self,
        system_prompt: Option<&str>,
        tools: Option<&[Tool]>,
    ) -> u64 {
        let system_tokens = system_prompt.map_or(0, |prompt| self.estimate_text_tokens(prompt) + 4);

        let tool_tokens = tools
            .unwrap_or_default()
            .iter()
            .map(|tool| {
                20 + self.estimate_text_tokens(&tool.name)
                    + self.estimate_text_tokens(&tool.description)
                    + self.estimate_json_tokens(&tool.parameters)
            })
            .sum::<u64>();

        system_tokens + tool_tokens
    }

    /// Estimate tokens using tokenizer-defined lexical boundaries.
    fn estimate_text_tokens(
        &self,
        text: &str,
    ) -> u64 {
        if text.is_empty() {
            return 0;
        }

        let Ok(encoding) = ESTIMATION_TOKENIZER.encode(text, false) else {
            // Encoding with the built-in tokenizer is expected to succeed.
            // Retain a conservative fallback in case that invariant changes.
            return (text.len() as u64).div_ceil(3);
        };

        encoding
            .get_offsets()
            .iter()
            .map(|(start, end)| {
                // Four bytes per subword is a conservative approximation for
                // the provider tokenizer after lexical splitting. Each
                // tokenizer-produced word or punctuation piece costs at least
                // one token.
                u64::try_from(end.saturating_sub(*start))
                    .unwrap_or(u64::MAX)
                    .div_ceil(4)
                    .max(1)
            })
            .sum()
    }

    /// Estimate the number of tokens in a serialized JSON value.
    fn estimate_json_tokens(
        &self,
        value: &serde_json::Value,
    ) -> u64 {
        self.estimate_text_tokens(&value.to_string())
    }
}

/// Estimate visual input from dimensions using OpenAI's documented tiled-image
/// approximation. If bytes cannot be decoded, reserve 1,000 tokens, the
/// conservative fallback used for opaque or provider-specific image inputs.
fn estimate_image_tokens(image: &ImageContent) -> u64 {
    let decoded = base64::engine::general_purpose::STANDARD.decode(&image.data);

    let Ok(bytes) = decoded else {
        return 1_000;
    };

    let Ok(image) = image::load_from_memory(&bytes) else {
        return 1_000;
    };

    let (width, height) = image.dimensions();

    let tiles_wide = u64::from(width).div_ceil(512).max(1);
    let tiles_high = u64::from(height).div_ceil(512).max(1);

    85 + tiles_wide.saturating_mul(tiles_high).saturating_mul(170)
}

/// Estimate the number of tokens in a message with the conservative fallback.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    TokenEstimator::fallback().estimate_message_tokens(message)
}

/// Estimate total tokens in a context with the conservative fallback.
pub fn estimate_context_tokens(context: &Context) -> u64 {
    let estimator = TokenEstimator::fallback();
    estimator.estimate_static_context_tokens(
        context.system_prompt.as_deref().map(String::as_str),
        context.tools.as_deref().map(Vec::as_slice),
    ) + context
        .messages
        .iter()
        .map(|message| estimator.estimate_message_tokens(message))
        .sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{Api, AssistantContent, StopReason, TextContent, UserMessage};

    #[test]
    fn fallback_estimator_handles_ascii_cjk_emoji_and_json() {
        let estimator = TokenEstimator::fallback();

        assert_eq!(estimator.estimate_text_tokens("Hello, world!"), 5);
        assert_eq!(estimator.estimate_text_tokens(""), 0);
        assert!(estimator.estimate_text_tokens("こんにちは") >= 4);
        assert!(estimator.estimate_text_tokens("😀😀😀") >= 3);
        assert!(estimator.estimate_json_tokens(&serde_json::json!({"command": "echo hello"})) > 0);
    }

    #[test]
    fn image_estimation_uses_decoded_dimensions_when_available() {
        // A one-pixel PNG exercises the dimension-aware tiled estimate rather
        // than the opaque-image fallback.
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let image = ImageContent::new(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            "image/png",
        );

        assert_eq!(estimate_image_tokens(&image), 255);
    }

    #[test]
    fn test_estimate_message_tokens_user() {
        let msg = crate::ai::types::Message::User(crate::ai::types::UserMessage::new(
            "Hello, this is a test message.",
        ));
        let tokens = estimate_message_tokens(&msg);

        assert!(tokens > 0);
        assert!(tokens < 100); // Should be a small number
    }

    #[test]
    fn test_estimate_message_tokens_assistant() {
        let msg = crate::ai::types::Message::Assistant(crate::ai::types::AssistantMessage::new(
            vec![AssistantContent::Text(TextContent::new(
                "I can help you with that.",
            ))],
            Api::OpenaiCompletions,
            "test",
            "test-model",
            StopReason::Stop,
        ));

        let tokens = estimate_message_tokens(&msg);

        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_context_tokens() {
        let context = Context::new(vec![Message::User(UserMessage::new("Hello"))])
            .with_system_prompt("You are a helpful assistant.");

        let tokens = estimate_context_tokens(&context);

        assert!(tokens > 0);
    }

    #[test]
    fn test_agent_state_display() {
        assert_eq!(format!("{}", AgentState::Idle), "idle");
        assert_eq!(format!("{}", AgentState::Streaming), "streaming");
        assert_eq!(format!("{}", AgentState::TurnComplete), "turn_complete");
    }
}
