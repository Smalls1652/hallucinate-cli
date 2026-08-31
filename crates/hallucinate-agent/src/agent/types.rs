//! Agent-specific types: states, events, and configuration.

use std::{fmt, sync::Arc};

use tokio::sync::Notify;

use crate::ai::types::{AssistantMessage, Context, Message, StopReason};

// ── Agent State ──────────────────────────────────────────────────────────────

/// Handle used by callers that do not own the running agent to request
/// cancellation (for example, the interactive UI while a task owns `Agent`).
#[derive(Clone, Debug)]
pub struct AgentCancellation {
    notify: Arc<Notify>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl AgentCancellation {
    pub(crate) fn new(notify: Arc<Notify>) -> Self {
        Self {
            notify,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Request cancellation of the active generation.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // `notify_one` retains a permit when cancellation happens just before
        // the async waiter is registered, avoiding a lost wake-up race.
        self.notify.notify_one();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn fresh() -> Self {
        Self::new(Arc::new(Notify::new()))
    }

    pub(crate) async fn cancelled(&self) {
        if !self.is_cancelled() {
            self.notify.notified().await;
        }
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
        new_state: AgentState,
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
        turn_number: u32,
        message: Box<AssistantMessage>,
    },

    /// Tool call detected in assistant response.
    ToolCallDetected {
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },

    /// Tool execution started.
    ToolExecutionStarted {
        tool_call_id: String,
        tool_name: String,
    },

    /// Tool execution completed.
    ToolExecutionCompleted {
        tool_call_id: String,
        tool_name: String,
        result: String,
        is_error: bool,
    },

    /// Tool execution failed.
    ToolExecutionFailed {
        tool_call_id: String,
        tool_name: String,
        error: String,
    },

    /// Turn completed successfully.
    TurnCompleted {
        turn_number: u32,
        stop_reason: StopReason,
    },

    /// Error occurred during agent operation.
    Error { message: String },

    /// Context compaction performed.
    CompactionPerformed {
        tokens_before: u64,
        tokens_after: u64,
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

            AgentEvent::ToolCallDetected { tool_name, .. } => {
                write!(f, "[tool_call] {tool_name}")
            },

            AgentEvent::ToolExecutionStarted { tool_name, .. } => {
                write!(f, "[tool_exec] starting {tool_name}")
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
    /// Whether to include tool definitions in context.
    pub include_tool_definitions: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 20,
            max_context_tokens: 128_000,
            enable_compaction: true,
            enable_tools: true,
            temperature: None,
            max_tokens: None,
            include_tool_definitions: true,
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
    pub fn new(_turn_number: u32) -> Self {
        Self {
            pending_tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

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
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

// ── Message History ──────────────────────────────────────────────────────────

/// Estimate the number of tokens in a message.
///
/// This is a rough approximation using character-based heuristics.
/// For more accurate estimates, a proper tokenizer should be used.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        crate::ai::types::Message::User(msg) => {
            estimate_text_tokens(&msg.content) + estimate_image_tokens(msg.images.len())
        },

        crate::ai::types::Message::Assistant(msg) => {
            let mut tokens: u64 = 0;
            for content in &msg.content {
                match content {
                    crate::ai::types::AssistantContent::Text(t) => {
                        tokens += estimate_text_tokens(&t.text);
                    },

                    crate::ai::types::AssistantContent::Thinking(t) => {
                        tokens += estimate_text_tokens(&t.thinking);
                    },

                    crate::ai::types::AssistantContent::ToolCall(tc) => {
                        // Tool calls have overhead
                        tokens += 10
                            + estimate_text_tokens(&tc.name)
                            + estimate_json_tokens(&tc.arguments);
                    },
                }
            }

            tokens
        },

        crate::ai::types::Message::ToolResult(msg) => {
            let mut tokens: u64 = 0;

            for content in &msg.content {
                tokens += estimate_text_tokens(&content.text);
            }

            tokens + estimate_image_tokens(msg.images.len())
        },
    }
}

/// Estimate image-token usage conservatively when exact image dimensions are
/// unavailable.
fn estimate_image_tokens(count: usize) -> u64 {
    count as u64 * 1_000
}

/// Estimate tokens from text using a simple heuristic (~4 chars per token).
fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }

    (text.chars().count() as u64).div_ceil(4)
}

/// Estimate tokens from JSON value.
fn estimate_json_tokens(value: &serde_json::Value) -> u64 {
    estimate_text_tokens(&value.to_string())
}

/// Estimate total tokens in a context.
pub fn estimate_context_tokens(context: &Context) -> u64 {
    let mut total: u64 = 0;

    // System prompt overhead
    if let Some(system) = &context.system_prompt {
        total += estimate_text_tokens(system) + 4; // system message overhead
    }

    // Message overhead
    for msg in &context.messages {
        total += estimate_message_tokens(msg);
    }

    // Tool definitions overhead
    if let Some(tools) = &context.tools {
        for tool in tools {
            total += 20
                + estimate_text_tokens(&tool.name)
                + estimate_text_tokens(&tool.description)
                + estimate_json_tokens(&tool.parameters);
        }
    }

    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{Api, AssistantContent, StopReason, TextContent, UserMessage};

    #[test]
    fn test_estimate_text_tokens() {
        // ~4 chars per token heuristic
        assert_eq!(estimate_text_tokens("Hello"), 2);
        assert_eq!(estimate_text_tokens("Hello, World!"), 4);
        assert_eq!(estimate_text_tokens(""), 0);
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
