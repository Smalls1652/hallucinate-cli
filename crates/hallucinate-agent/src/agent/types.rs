//! Agent-specific types: states, events, and configuration.

use std::fmt;

use crate::ai::types::{AssistantMessage, RequestCancellation, StopReason, ToolCall};

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
        /// Number of summaries added during compaction.
        summaries_added: u32,

        /// Provider-reported context tokens immediately before compaction.
        ///
        /// This is retained for session displays after resuming. The agent does
        /// not reuse it for compaction decisions because the summary replaces
        /// part of the context.
        context_token_count: u64,
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
                summaries_added,
                context_token_count,
            } => {
                write!(
                    f,
                    "[compaction] added {summaries_added} summary ({context_token_count} tokens)"
                )
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

    /// Tools that are enabled.
    pub enabled_tools: Vec<String>,

    /// Tools that are disabled.
    pub disabled_tools: Vec<String>,

    /// Default temperature for LLM calls.
    pub temperature: Option<f64>,

    /// Default max tokens for LLM calls.
    pub max_tokens: Option<u32>,

    /// Requested model reasoning effort.
    pub reasoning_effort: Option<crate::ai::types::ReasoningEffort>,

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
            enabled_tools: vec![],
            disabled_tools: vec![],
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
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
