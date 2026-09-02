//! Agent struct: state machine, event subscription, and tool management.
//!
//! The [`Agent`] struct manages the lifecycle of multi-turn conversations
//! with an LLM provider, including state transitions, event emission,
//! and tool execution coordination.

use std::{
    cell::{Ref, RefCell},
    sync::Arc,
    time::Instant,
};

use futures::{StreamExt, stream};
use tokio::sync::{Notify, mpsc};

use crate::{
    agent::{
        skills::Skill,
        tools::{ToolRegistry, types::Tool as AgentTool},
        types::{
            AgentCancellation,
            AgentConfig,
            AgentEvent,
            AgentState,
            PendingToolCall,
            TokenEstimator,
            TurnContext,
        },
    },
    ai::{
        provider::Provider,
        types::{AssistantContent, Context, Message, Model, StopReason, StreamOptions, Tool},
    },
};

/// Immutable request data shared by every turn until agent configuration
/// changes. Keeping it separate prevents prompt-file reads and tool-schema
/// conversion during ordinary context-pressure checks.
#[derive(Debug, Clone)]
struct ContextBase {
    system_prompt: Arc<String>,
    tools: Option<Arc<Vec<Tool>>>,
    static_token_cost: u64,
}

/// The main agent struct that manages multi-turn conversations.
///
/// The agent coordinates between the LLM provider and tools, handling
/// the turn lifecycle: request response → detect tool calls → execute tools →
/// send results → next turn.
pub struct Agent {
    /// Agent configuration.
    config: AgentConfig,
    /// Current agent state.
    state: AgentState,
    /// The LLM provider.
    provider: Box<dyn Provider>,
    /// The model to use.
    model: Model,
    /// Registered tools.
    tool_registry: ToolRegistry,
    /// Message history.
    messages: Vec<Message>,
    /// Cached token cost for each item in `messages`.
    message_token_costs: Vec<u64>,
    /// Sum of `message_token_costs`, maintained on every history mutation.
    message_token_total: u64,
    /// Token estimator used for the active model.
    token_estimator: TokenEstimator,
    /// Immutable request data, built lazily after configuration is complete.
    context_base: RefCell<Option<ContextBase>>,
    /// Explicit system-prompt override.
    system_prompt: Option<String>,
    /// Skills available for model-directed loading.
    skills: Vec<Skill>,
    /// Event subscribers.
    event_senders: Vec<mpsc::UnboundedSender<AgentEvent>>,
    /// Current turn context.
    turn_context: Option<TurnContext>,
    /// Current working directory.
    cwd: Option<String>,
    /// Shared cancellation notification for the active turn.
    cancellation: AgentCancellation,
}

impl Agent {
    /// Create a new agent with the given provider and model.
    pub fn new(
        provider: Box<dyn Provider>,
        model: Model,
        config: AgentConfig,
    ) -> Self {
        let token_estimator = TokenEstimator::for_model(&model);

        Self {
            config,
            state: AgentState::Idle,
            provider,
            model,
            tool_registry: ToolRegistry::new(),
            messages: Vec::new(),
            message_token_costs: Vec::new(),
            message_token_total: 0,
            token_estimator,
            context_base: RefCell::new(None),
            system_prompt: None,
            skills: Vec::new(),
            event_senders: Vec::new(),
            turn_context: None,
            cwd: None,
            cancellation: AgentCancellation::new(Arc::new(Notify::new())),
        }
    }

    /// Create a new agent with built-in tools and an image resize policy.
    pub fn with_all_tools_and_image_resize(
        provider: Box<dyn Provider>,
        model: Model,
        config: AgentConfig,
        auto_resize_images: bool,
    ) -> Self {
        let mut agent = Self::new(provider, model, config);
        agent.tool_registry = ToolRegistry::with_all_builtins_and_image_resize(auto_resize_images);
        agent.invalidate_context_base();

        agent
    }

    /// Create a new agent with built-in tools and a configured shell
    /// executable.
    pub fn with_all_tools_and_image_resize_and_shell(
        provider: Box<dyn Provider>,
        model: Model,
        config: AgentConfig,
        auto_resize_images: bool,
        shell: Option<&str>,
    ) -> Self {
        if shell.is_none() {
            return Self::with_all_tools_and_image_resize(
                provider,
                model,
                config,
                auto_resize_images,
            );
        }

        let mut agent = Self::new(provider, model, config);
        agent.tool_registry =
            ToolRegistry::with_all_builtins_and_image_resize_and_shell(auto_resize_images, shell);
        agent.invalidate_context_base();

        agent
    }

    /// Set the system prompt.
    pub fn with_system_prompt(
        mut self,
        prompt: String,
    ) -> Self {
        self.system_prompt = Some(prompt);
        self.invalidate_context_base();

        self
    }

    /// Set the skills made available to the model.
    pub fn with_skills(
        mut self,
        skills: Vec<Skill>,
    ) -> Self {
        self.skills = skills;
        self.invalidate_context_base();

        self
    }

    /// Set the current working directory.
    pub fn with_cwd(
        mut self,
        cwd: String,
    ) -> Self {
        self.cwd = Some(cwd);
        self.invalidate_context_base();

        self
    }

    /// Replace the explicit system-prompt override for future requests.
    pub fn set_system_prompt(
        &mut self,
        prompt: Option<String>,
    ) {
        self.system_prompt = prompt;
        self.invalidate_context_base();
    }

    /// Replace the skills available for model-directed loading.
    pub fn set_skills(
        &mut self,
        skills: Vec<Skill>,
    ) {
        self.skills = skills;
        self.invalidate_context_base();
    }

    /// Add a model-invocable tool and invalidate cached tool schemas.
    pub fn add_tool(
        &mut self,
        tool: Arc<dyn AgentTool + Send + Sync>,
    ) {
        self.tool_registry.add_tool(tool);
        self.invalidate_context_base();
    }

    /// Update the working directory when resuming a session.
    pub fn set_cwd(
        &mut self,
        cwd: String,
    ) {
        self.cwd = Some(cwd);
        self.invalidate_context_base();
    }

    /// Reload `SYSTEM.md` and `AGENTS.md` for the current directory.
    ///
    /// Prompt files are otherwise read only when cached context data is
    /// invalidated. This explicit operation makes external prompt edits
    /// visible without adding filesystem metadata checks to every turn.
    pub fn reload_system_prompt(&mut self) {
        self.invalidate_context_base();
        self.refresh_context_base();
    }

    /// Return the working directory used for project-local context files.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Return a cancellation handle that remains usable while this agent is
    /// owned by a background task.
    pub fn cancellation(&self) -> AgentCancellation {
        self.cancellation.clone()
    }

    /// Subscribe to agent events.
    ///
    /// Returns a receiver channel for [`AgentEvent`]s.
    pub fn subscribe(&mut self) -> mpsc::UnboundedReceiver<AgentEvent> {
        let (tx, rx) = mpsc::unbounded_channel();

        self.event_senders.push(tx);

        rx
    }

    /// Get the current state.
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Replace the provider and model for the next turn. Callers must only
    /// invoke this while the agent is idle; changing a provider during a
    /// request would invalidate the in-flight stream.
    pub fn set_model(
        &mut self,
        provider: Box<dyn Provider>,
        model: Model,
        max_tokens: Option<u32>,
    ) {
        self.provider = provider;
        self.model = model.clone();
        self.config.max_context_tokens = model.context_window;
        self.config.max_tokens = max_tokens;
        self.token_estimator = TokenEstimator::for_model(&model);
        self.recalculate_message_token_costs();
        self.invalidate_context_base();
    }

    /// Get the message history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Replace conversation history when resuming a persisted session.
    /// Incomplete assistant tool-use messages are rejected because sending them
    /// without their matching tool results produces an invalid provider
    /// context.
    pub fn load_history(
        &mut self,
        messages: Vec<Message>,
    ) -> Result<(), anyhow::Error> {
        for (index, message) in messages.iter().enumerate() {
            let Message::Assistant(assistant) = message else {
                continue;
            };

            let tool_calls: Vec<_> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantContent::ToolCall(call) => Some(&call.id),
                    _ => None,
                })
                .collect();

            for (offset, tool_call_id) in tool_calls.iter().enumerate() {
                let result = messages.get(index + 1 + offset);
                let matches_call = matches!(
                    result,
                    Some(Message::ToolResult(result)) if result.tool_call_id == **tool_call_id
                );

                if !matches_call {
                    return Err(anyhow::anyhow!(
                        "persisted history has tool call '{}' without its matching following tool result",
                        tool_call_id
                    ));
                }
            }
        }

        self.messages = messages;
        self.recalculate_message_token_costs();
        self.turn_context = None;
        self.cancellation = AgentCancellation::fresh();

        if self.state != AgentState::Idle {
            self.set_state(AgentState::Idle);
        }

        Ok(())
    }

    /// Emit an event to all subscribers.
    fn emit(
        &mut self,
        event: AgentEvent,
    ) {
        // Clone for display
        let _ = format!("{}", event);

        // Send to all subscribers
        self.event_senders
            .retain(|sender| sender.send(event.clone()).is_ok());
    }

    /// Update the agent state and emit a state change event.
    fn set_state(
        &mut self,
        new_state: AgentState,
    ) {
        let old_state = std::mem::replace(&mut self.state, new_state.clone());

        self.emit(AgentEvent::StateChanged {
            new_state,
            old_state,
        });
    }

    /// Mark immutable request data as stale after an agent configuration
    /// change.
    fn invalidate_context_base(&mut self) {
        *self.context_base.borrow_mut() = None;
    }

    /// Build the immutable request data on demand.
    fn refresh_context_base(&self) {
        let started_at = Instant::now();
        let prompt_refresh_started_at = Instant::now();
        let cwd = self.cwd.as_deref().map(std::path::Path::new);
        let mut system_prompt = crate::agent::system_prompt::build_system_prompt_for_cwd(
            self.system_prompt.as_deref(),
            cwd,
        );

        if self.config.enable_tools && self.tool_registry.get_tool("read").is_some() {
            system_prompt.push_str(&crate::agent::skills::format_skills_for_prompt(
                &self.skills,
            ));
        }
        let prompt_refresh_ms = prompt_refresh_started_at.elapsed().as_millis();

        let tools = (self.config.enable_tools
            && self.config.include_tool_definitions
            && !self.tool_registry.is_empty())
        .then(|| self.tool_registry.to_ai_tools());
        let token_estimation_started_at = Instant::now();
        let static_token_cost = self
            .token_estimator
            .estimate_static_context_tokens(Some(&system_prompt), tools.as_deref());
        let token_estimation_ms = token_estimation_started_at.elapsed().as_millis();

        tracing::debug!(
            elapsed_ms = started_at.elapsed().as_millis(),
            prompt_refresh_ms,
            token_estimation_ms,
            system_prompt_chars = system_prompt.len(),
            tool_count = tools.as_ref().map_or(0, Vec::len),
            static_token_cost,
            "refreshed cached agent context base"
        );

        *self.context_base.borrow_mut() = Some(ContextBase {
            system_prompt: Arc::new(system_prompt),
            tools: tools.map(Arc::new),
            static_token_cost,
        });
    }

    /// Return cached immutable request data, refreshing it after invalidation.
    fn context_base(&self) -> Ref<'_, ContextBase> {
        if self.context_base.borrow().is_none() {
            self.refresh_context_base();
        }

        Ref::map(self.context_base.borrow(), |context_base| {
            context_base
                .as_ref()
                .expect("context base is initialized by refresh_context_base")
        })
    }

    /// Build the context for one provider request.
    fn build_context(&mut self) -> Context {
        let started_at = Instant::now();
        let context_base = self.context_base();
        let context = Context {
            system_prompt: Some(context_base.system_prompt.clone()),
            messages: self.messages.clone(),
            tools: context_base.tools.clone(),
        };

        tracing::debug!(
            elapsed_ms = started_at.elapsed().as_millis(),
            message_count = context.messages.len(),
            "assembled request context"
        );

        context
    }

    /// Recalculate cached message costs after bulk history replacement.
    fn recalculate_message_token_costs(&mut self) {
        let started_at = Instant::now();
        self.message_token_costs = self
            .messages
            .iter()
            .map(|message| self.token_estimator.estimate_message_tokens(message))
            .collect();
        self.message_token_total = self.message_token_costs.iter().sum();

        tracing::debug!(
            elapsed_ms = started_at.elapsed().as_millis(),
            message_count = self.messages.len(),
            message_token_total = self.message_token_total,
            "recalculated cached message token costs"
        );
    }

    /// Add a message and maintain its cached token cost.
    fn push_message(
        &mut self,
        message: Message,
    ) {
        let token_cost = self.token_estimator.estimate_message_tokens(&message);
        self.messages.push(message);
        self.message_token_costs.push(token_cost);
        self.message_token_total = self.message_token_total.saturating_add(token_cost);
    }

    /// Add a user message to the history.
    pub fn add_user_message(
        &mut self,
        content: impl Into<String>,
    ) {
        let msg = Message::User(crate::ai::types::UserMessage::new(content));

        self.push_message(msg);
    }

    /// Add an assistant message to the history.
    fn add_assistant_message(
        &mut self,
        message: crate::ai::types::AssistantMessage,
    ) {
        self.push_message(Message::Assistant(message.clone()));
    }

    /// Add a tool result message to the history.
    fn add_tool_result_message(
        &mut self,
        result: &crate::agent::tools::ToolResult,
    ) {
        self.push_message(crate::agent::tools::types::tool_result_to_message_for_context(result));
    }

    /// Extract tool calls from an assistant message.
    fn extract_tool_calls(
        &self,
        message: &crate::ai::types::AssistantMessage,
    ) -> Vec<PendingToolCall> {
        let mut calls = Vec::new();

        for content in &message.content {
            if let AssistantContent::ToolCall(tc) = content {
                calls.push(PendingToolCall {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                });
            }
        }

        calls
    }

    /// Process a single assistant response turn.
    async fn process_turn(
        &mut self,
        turn_number: u32,
        context: Context,
    ) -> Result<StopReason, anyhow::Error> {
        // Emit turn started event
        self.emit(AgentEvent::TurnStarted { turn_number });
        self.turn_context = Some(TurnContext::new(turn_number));

        // Build stream options
        let mut options = StreamOptions::new();

        if let Some(temp) = self.config.temperature {
            options = options.with_temperature(temp);
        }

        if let Some(max) = self.config.max_tokens {
            options = options.with_max_tokens(max);
        }

        // Stream the response
        self.set_state(AgentState::Streaming);

        let mut stream = tokio::select! {
            result = self.provider.stream(&self.model, &context, &options) => result?,
            _ = self.cancellation.cancelled() => {
                return Err(anyhow::anyhow!("Agent generation was cancelled"));
            }
        };

        let mut assistant_message = None;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = self.cancellation.cancelled() => {
                    return Err(anyhow::anyhow!("Agent generation was cancelled"));
                }
            };

            let Some(event) = event else {
                break;
            };

            match &event {
                crate::ai::streaming::AssistantMessageEvent::TextDelta {
                    content_index,
                    delta,
                    ..
                } => {
                    let _ = content_index;

                    self.emit(AgentEvent::AssistantTextDelta {
                        delta: delta.clone(),
                    });
                },

                crate::ai::streaming::AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta,
                    ..
                } => {
                    let _ = content_index;

                    self.emit(AgentEvent::AssistantThinkingDelta {
                        delta: delta.clone(),
                    });
                },

                crate::ai::streaming::AssistantMessageEvent::ToolcallStart {
                    content_index,
                    tool_call,
                } => {
                    self.emit(AgentEvent::ToolCallStarted {
                        content_index: *content_index,
                        tool_call: tool_call.clone(),
                    });
                },

                crate::ai::streaming::AssistantMessageEvent::ToolcallDelta {
                    content_index,
                    tool_call,
                    ..
                } => {
                    self.emit(AgentEvent::ToolCallDelta {
                        content_index: *content_index,
                        tool_call: tool_call.clone(),
                    });
                },

                crate::ai::streaming::AssistantMessageEvent::Done { message, .. } => {
                    assistant_message = Some(message.clone());
                },

                crate::ai::streaming::AssistantMessageEvent::Error { error, .. } => {
                    return Err(anyhow::anyhow!(
                        "LLM error: {}",
                        error.error_message.as_deref().unwrap_or("unknown error")
                    ));
                },

                crate::ai::streaming::AssistantMessageEvent::ToolcallEnd { tool_call, .. } => {
                    self.emit(AgentEvent::ToolCallDetected {
                        tool_call_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        arguments: tool_call.arguments.clone(),
                    });
                },

                _ => {},
            }
        }

        let message =
            assistant_message.ok_or_else(|| anyhow::anyhow!("Stream ended without a message"))?;

        // Add to history and notify subscribers after the complete message is
        // available. The UI uses the text-delta events above for incremental
        // rendering, then this event to finalize its content and tool calls.
        self.add_assistant_message(message.clone());
        self.emit(AgentEvent::AssistantMessageCompleted {
            turn_number,
            message: Box::new(message.clone()),
        });

        // Check if there are tool calls to execute
        let tool_calls = self.extract_tool_calls(&message);
        if let Some(turn_context) = self.turn_context.as_mut() {
            for call in &tool_calls {
                turn_context.add_pending_tool_call(
                    call.tool_call_id.clone(),
                    call.tool_name.clone(),
                    call.arguments.clone(),
                );
            }
        }

        if tool_calls.is_empty() {
            // No tool calls, turn is complete
            self.set_state(AgentState::TurnComplete);

            self.emit(AgentEvent::TurnCompleted {
                turn_number,
                stop_reason: message.stop_reason.clone(),
            });

            return Ok(message.stop_reason);
        }

        // Execute tool calls
        self.set_state(AgentState::ToolExecuting);

        let concurrency_limit = self.config.max_concurrent_tool_calls.max(1);
        let cwd = self.cwd.clone();

        tracing::info!(
            tool_call_count = tool_calls.len(),
            concurrency_limit,
            "executing tool calls"
        );

        // Emit all starts before work begins so the user-visible lifecycle is
        // stable even though calls are allowed to complete out of order.
        let mut tool_jobs = Vec::with_capacity(tool_calls.len());
        for (call_index, call) in tool_calls.into_iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStarted {
                tool_call_id: call.tool_call_id.clone(),
                tool_name: call.tool_name.clone(),
            });

            let tool = self.tool_registry.get_tool(&call.tool_name).cloned();
            let cwd = cwd.clone();

            tool_jobs.push(async move {
                let started_at = Instant::now();
                let result = match tool {
                    Some(tool) => tool.execute(&call.arguments, cwd.as_deref()).await,
                    None => Err(anyhow::anyhow!("Unknown tool: {}", call.tool_name)),
                };

                let mut result = result.unwrap_or_else(|error| {
                    crate::agent::tools::ToolResult::error(
                        &call.tool_call_id,
                        &call.tool_name,
                        format!("Tool execution failed: {error}"),
                    )
                });
                // Built-in tools do not know the provider-issued ID. The
                // agent is the boundary that associates a result with its
                // model call.
                result.tool_call_id.clone_from(&call.tool_call_id);
                result.tool_name.clone_from(&call.tool_name);

                tracing::info!(
                    tool_call_index = call_index,
                    tool_name = %call.tool_name,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    is_error = result.is_error,
                    "tool call completed"
                );

                (call_index, call, result)
            });
        }
        let mut pending_jobs = stream::iter(tool_jobs).buffer_unordered(concurrency_limit);
        let mut completed_jobs = Vec::new();

        while let Some(job) = tokio::select! {
            job = pending_jobs.next() => job,
            _ = self.cancellation.cancelled() => {
                // Dropping the buffered futures prevents queued calls from
                // starting and releases in-flight async tool work.
                return Err(anyhow::anyhow!("Agent generation was cancelled"));
            }
        } {
            completed_jobs.push(job);
        }

        // Provider protocols require tool result messages in the same order
        // as their calls, regardless of completion order.
        completed_jobs.sort_unstable_by_key(|(call_index, _, _)| *call_index);
        let mut results = Vec::with_capacity(completed_jobs.len());

        for (_, call, tool_result) in completed_jobs {
            if let Some(turn_context) = self.turn_context.as_mut() {
                turn_context.remove_completed_tool_call(&call.tool_call_id);
                turn_context.tool_results.push(tool_result.clone());
            }

            if tool_result.is_error {
                self.emit(AgentEvent::ToolExecutionFailed {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.tool_name,
                    error: tool_result.content.clone(),
                });
            } else {
                self.emit(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.tool_name,
                    result: tool_result.content.clone(),
                    is_error: false,
                });
            }

            results.push(tool_result);
        }

        // Add tool results to history in original model-call order.
        for result in &results {
            self.add_tool_result_message(result);
        }

        // Continue to next turn (tools were used, so we loop back)
        self.set_state(AgentState::Idle);

        // Return ToolUse to indicate we should continue
        Ok(StopReason::ToolUse)
    }

    /// Run the agent loop for a single user message.
    ///
    /// This processes turns until the assistant responds without tool calls,
    /// or until max_turns is reached. A fresh cancellation handle is installed
    /// after every run so the same agent can safely accept another prompt.
    pub async fn run(&mut self) -> Result<StopReason, anyhow::Error> {
        if self.cancellation.is_cancelled() {
            self.set_state(AgentState::Aborted);
            self.emit(AgentEvent::Aborted);
            self.cancellation = AgentCancellation::fresh();

            return Err(anyhow::anyhow!("Agent generation was cancelled"));
        }

        let result = self.run_loop().await;
        self.cancellation = AgentCancellation::fresh();

        result
    }

    /// Run the agent loop, processing turns until the assistant responds
    /// without tool calls, max turns is reached, or an error occurs.
    async fn run_loop(&mut self) -> Result<StopReason, anyhow::Error> {
        let mut turn_number = 1;

        loop {
            if self.cancellation.is_cancelled() {
                self.set_state(AgentState::Aborted);
                self.emit(AgentEvent::Aborted);

                return Err(anyhow::anyhow!("Agent generation was cancelled"));
            }

            // Check max turns
            if turn_number > self.config.max_turns {
                let message = format!(
                    "Reached maximum number of turns ({})",
                    self.config.max_turns
                );

                self.set_state(AgentState::Error(message.clone()));
                self.emit(AgentEvent::Error {
                    message: message.clone(),
                });

                return Err(anyhow::anyhow!(message));
            }

            // Context pressure is an O(1) cached static cost plus cached
            // history total. Only build the owned provider context once after
            // any required compaction has completed.
            if self.config.enable_compaction {
                let token_count = self
                    .context_base()
                    .static_token_cost
                    .saturating_add(self.message_token_total);

                if token_count > u64::from(self.config.max_context_tokens)
                    && let Err(error) = self.compact_context(false)
                {
                    let message = error.to_string();

                    self.set_state(AgentState::Error(message.clone()));
                    self.emit(AgentEvent::Error { message });

                    return Err(error);
                }
            }

            let context = self.build_context();

            // Process the turn. Provider and stream failures are observable
            // state transitions, not merely errors returned to the caller.
            let stop_reason = match self.process_turn(turn_number, context).await {
                Ok(reason) => reason,
                Err(error) => {
                    if self.cancellation.is_cancelled() {
                        self.set_state(AgentState::Aborted);
                        self.emit(AgentEvent::Aborted);
                    } else {
                        let message = error.to_string();
                        self.set_state(AgentState::Error(message.clone()));
                        self.emit(AgentEvent::Error { message });
                    }

                    return Err(error);
                },
            };

            if stop_reason != StopReason::ToolUse {
                return Ok(stop_reason);
            }

            turn_number += 1;
        }
    }

    /// Compact the context by summarizing older messages.
    ///
    /// Unlike automatic compaction, an explicit request always creates a
    /// compacted history when there are messages to summarize.
    pub fn compact(&mut self) -> Result<(), anyhow::Error> {
        self.compact_context(true)
    }

    /// Compact the message context by summarizing older messages.
    ///
    /// Automatic compaction only modifies history once it exceeds the message
    /// budget. Manual compaction reserves one third of the current message
    /// history for a summary and retains the most recent messages, so
    /// `/compact` has an observable effect before the context window is full.
    fn compact_context(
        &mut self,
        force: bool,
    ) -> Result<(), anyhow::Error> {
        let started_at = Instant::now();
        let non_message_tokens = self.context_base().static_token_cost;
        let before_tokens = non_message_tokens.saturating_add(self.message_token_total);

        let maximum_message_budget = u64::from(self.config.max_context_tokens)
            .saturating_sub(non_message_tokens)
            .min(u64::from(self.config.max_context_tokens));
        let message_budget = if force {
            // Retain approximately two thirds of the existing message
            // history. The lower bound lets the compactor safely represent a
            // small conversation without returning an invalid tiny budget.
            self.message_token_total
                .saturating_mul(2)
                .saturating_div(3)
                .max(32)
                .min(maximum_message_budget)
        } else {
            maximum_message_budget
        };

        if message_budget > u64::from(u32::MAX) {
            return Err(anyhow::anyhow!("context budget is too large to compact"));
        }

        // Compact only the message portion. The system prompt and tool
        // definitions remain in the context and consume their own budget.
        let (remaining, summaries) = crate::agent::compaction::compact_messages_with_costs(
            &self.messages,
            &self.message_token_costs,
            message_budget as u32,
        )?;

        // `compact_messages` returns retained history and summary text, not
        // pre-inserted messages. Insert the summary exactly once before the
        // chronological retained history.
        self.messages = summaries
            .iter()
            .map(|summary| {
                Message::User(crate::ai::types::UserMessage::new(format!(
                    "[Context Summary]\n{summary}"
                )))
            })
            .chain(remaining)
            .collect();
        self.recalculate_message_token_costs();

        let after_tokens = non_message_tokens.saturating_add(self.message_token_total);
        tracing::debug!(
            elapsed_ms = started_at.elapsed().as_millis(),
            tokens_before = before_tokens,
            tokens_after = after_tokens,
            "compacted cached agent context"
        );

        self.emit(AgentEvent::CompactionPerformed {
            tokens_before: before_tokens,
            tokens_after: after_tokens,
            summaries_added: summaries.len() as u32,
        });

        Ok(())
    }

    /// Get the estimated token count of the current context.
    pub fn estimate_tokens(&self) -> u64 {
        self.context_base()
            .static_token_cost
            .saturating_add(self.message_token_total)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use futures::Future;
    use tokio::sync::Barrier;

    use super::*;
    use crate::{
        agent::tools::{ToolResult, types::Tool},
        ai::{
            providers::faux::{FauxProvider, FauxResponse},
            streaming::{AssistantMessageEvent, create_assistant_message_event_stream},
            types::{Api, AssistantMessage, Model, StopReason, ToolCall},
        },
    };

    #[derive(Debug)]
    struct DelayedTool {
        barrier: Arc<Barrier>,
        active_calls: Arc<AtomicUsize>,
        peak_calls: Arc<AtomicUsize>,
    }

    impl Tool for DelayedTool {
        fn name(&self) -> &str {
            "delayed"
        }

        fn description(&self) -> &str {
            "A controlled test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            arguments: &'a serde_json::Value,
            _cwd: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>> {
            Box::pin(async move {
                let active = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak_calls.fetch_max(active, Ordering::SeqCst);
                self.barrier.wait().await;

                let delay_ms = arguments
                    .get("delay_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                self.active_calls.fetch_sub(1, Ordering::SeqCst);

                Ok(ToolResult::success("", "", delay_ms.to_string()))
            })
        }
    }

    #[derive(Debug, Default)]
    struct TwoTurnToolProvider {
        call_count: AtomicUsize,
    }

    impl Provider for TwoTurnToolProvider {
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: &StreamOptions,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Future<
                        Output = crate::ai::provider::ProviderResult<
                            crate::ai::streaming::AssistantMessageEventStream,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            let call_number = self.call_count.fetch_add(1, Ordering::SeqCst);
            let api = model.api.clone();
            let model_id = model.id.clone();

            Box::pin(async move {
                let stream = create_assistant_message_event_stream();
                stream.push(AssistantMessageEvent::Start).await.unwrap();
                let (content, reason) = if call_number == 0 {
                    (
                        vec![
                            AssistantContent::ToolCall(ToolCall::new(
                                "call-1",
                                "delayed",
                                serde_json::json!({"delay_ms": 30}),
                            )),
                            AssistantContent::ToolCall(ToolCall::new(
                                "call-2",
                                "delayed",
                                serde_json::json!({"delay_ms": 1}),
                            )),
                        ],
                        StopReason::ToolUse,
                    )
                } else {
                    (
                        vec![AssistantContent::Text(crate::ai::types::TextContent::new(
                            "done",
                        ))],
                        StopReason::Stop,
                    )
                };
                let message = AssistantMessage::new(content, api, "test", model_id, reason.clone());
                stream
                    .push(AssistantMessageEvent::Done { reason, message })
                    .await
                    .unwrap();
                Ok(stream)
            })
        }
    }

    /// Return a small test model for use in unit tests.
    fn test_model() -> Model {
        Model::new(
            "test-model",
            "Test Model",
            Api::OpenaiCompletions,
            "test",
            "http://localhost:0",
            false,
            128_000,
            16_384,
        )
    }

    #[tokio::test]
    async fn emits_text_deltas_and_a_completed_message() {
        let provider = FauxProvider::new();

        provider.set_responses(vec![FauxResponse::Text(
            "Streaming text".to_string(),
            StopReason::Stop,
        )]);

        let model = test_model();
        let mut agent = Agent::new(Box::new(provider), model, AgentConfig::default());
        let mut events = agent.subscribe();

        agent.add_user_message("Say something");
        agent.run().await.unwrap();

        let mut saw_delta = false;
        let mut saw_completed_message = false;

        while let Ok(event) = events.try_recv() {
            match event {
                AgentEvent::AssistantTextDelta { delta, .. } => {
                    saw_delta |= delta == "Streaming text";
                },
                AgentEvent::AssistantMessageCompleted { message, .. } => {
                    saw_completed_message |= matches!(
                        message.content.as_slice(),
                        [AssistantContent::Text(text)] if text.text == "Streaming text"
                    );
                },
                _ => {},
            }
        }

        assert!(saw_delta);
        assert!(saw_completed_message);
    }

    #[tokio::test]
    async fn emits_tool_call_start_before_the_completed_assistant_message() {
        let provider = FauxProvider::new();
        provider.set_responses(vec![
            FauxResponse::Text("done".to_string(), StopReason::Stop),
            FauxResponse::ToolCall(
                "call-1".to_string(),
                "bash".to_string(),
                serde_json::json!({ "command": "printf hello" }),
                StopReason::ToolUse,
            ),
        ]);

        let model = test_model();
        let mut agent = Agent::with_all_tools_and_image_resize(
            Box::new(provider),
            model,
            AgentConfig::default(),
            true,
        );
        let mut events = agent.subscribe();
        agent.add_user_message("run a command");

        assert_eq!(agent.run().await.unwrap(), StopReason::Stop);

        let events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let tool_call_start = events.iter().position(|event| {
            matches!(event, AgentEvent::ToolCallStarted { tool_call, .. }
                if tool_call.id == "call-1"
                    && tool_call.name == "bash"
                    && tool_call.arguments == serde_json::json!({ "command": "printf hello" }))
        });
        let completed_message = events.iter().position(|event| {
            matches!(event, AgentEvent::AssistantMessageCompleted { message, .. }
                if matches!(message.content.as_slice(), [AssistantContent::ToolCall(call)] if call.id == "call-1"))
        });

        assert!(tool_call_start.is_some());
        assert!(completed_message.is_some());
        assert!(tool_call_start < completed_message);
    }

    #[tokio::test]
    async fn cancellation_handle_transitions_agent_to_aborted() {
        let model = test_model();
        let mut agent = Agent::new(Box::new(FauxProvider::new()), model, AgentConfig::default());
        let cancellation = agent.cancellation();
        cancellation.cancel();

        assert!(agent.run().await.is_err());
        assert_eq!(agent.state(), &AgentState::Aborted);
    }

    #[tokio::test]
    async fn completed_runs_replace_the_cancellation_handle() {
        let provider = FauxProvider::new();
        provider.set_responses(vec![
            FauxResponse::Text("second".to_string(), StopReason::Stop),
            FauxResponse::Text("first".to_string(), StopReason::Stop),
        ]);

        let model = test_model();
        let mut agent = Agent::new(Box::new(provider), model, AgentConfig::default());
        let first_handle = agent.cancellation();

        agent.add_user_message("first prompt");
        agent.run().await.unwrap();
        first_handle.cancel();
        agent.add_user_message("second prompt");

        assert_eq!(agent.run().await.unwrap(), StopReason::Stop);
        assert_eq!(agent.state(), &AgentState::TurnComplete);
    }

    #[tokio::test]
    async fn provider_failures_emit_error_and_update_state() {
        let provider = FauxProvider::new();
        provider.set_responses(vec![FauxResponse::Error("provider failed".to_string())]);

        let model = test_model();
        let mut agent = Agent::new(Box::new(provider), model, AgentConfig::default());
        let mut events = agent.subscribe();

        agent.add_user_message("fail");

        assert!(agent.run().await.is_err());
        assert!(
            matches!(agent.state(), AgentState::Error(message) if message.contains("provider failed"))
        );

        let mut saw_error = false;

        while let Ok(event) = events.try_recv() {
            saw_error |= matches!(event, AgentEvent::Error { message } if message.contains("provider failed"));
        }

        assert!(saw_error);
    }

    #[tokio::test]
    async fn max_turns_stops_an_unbounded_tool_sequence() {
        let provider = FauxProvider::new();
        provider.set_responses(vec![FauxResponse::ToolCall(
            "call-1".to_string(),
            "bash".to_string(),
            serde_json::json!({ "command": "printf hello" }),
            StopReason::ToolUse,
        )]);

        let model = test_model();
        let config = AgentConfig {
            max_turns: 1,
            ..AgentConfig::default()
        };

        let mut agent =
            Agent::with_all_tools_and_image_resize(Box::new(provider), model, config, true);
        agent.add_user_message("keep going");

        let result = agent.run().await;

        assert!(result.is_err());
        assert!(matches!(agent.state(), AgentState::Error(message) if message.contains("maximum")));
    }

    #[tokio::test]
    async fn executes_tool_calls_and_continues_to_the_next_turn() {
        let provider = crate::ai::providers::faux::FauxProvider::new();
        provider.set_responses(vec![
            FauxResponse::Text("The command succeeded.".to_string(), StopReason::Stop),
            FauxResponse::ToolCall(
                "call-1".to_string(),
                "bash".to_string(),
                serde_json::json!({ "command": "printf hello" }),
                StopReason::ToolUse,
            ),
        ]);

        let model = test_model();

        let mut agent = Agent::with_all_tools_and_image_resize(
            Box::new(provider),
            model,
            AgentConfig::default(),
            true,
        );
        agent.add_user_message("Run the command and report back");

        assert_eq!(agent.run().await.unwrap(), StopReason::Stop);
        assert_eq!(agent.messages().len(), 4);
        assert!(matches!(agent.messages()[1], Message::Assistant(_)));
        assert!(matches!(agent.messages()[2], Message::ToolResult(_)));
        assert!(matches!(agent.messages()[3], Message::Assistant(_)));
    }

    #[tokio::test]
    async fn runs_tool_calls_concurrently_and_records_results_in_call_order() {
        let active_calls = Arc::new(AtomicUsize::new(0));
        let peak_calls = Arc::new(AtomicUsize::new(0));
        let tool = DelayedTool {
            barrier: Arc::new(Barrier::new(2)),
            active_calls,
            peak_calls: peak_calls.clone(),
        };
        let config = AgentConfig {
            max_concurrent_tool_calls: 2,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(
            Box::new(TwoTurnToolProvider::default()),
            test_model(),
            config,
        );
        agent.add_tool(Arc::new(tool));
        let mut events = agent.subscribe();
        agent.add_user_message("run both calls");

        assert_eq!(agent.run().await.unwrap(), StopReason::Stop);
        assert_eq!(peak_calls.load(Ordering::SeqCst), 2);

        let result_ids = agent
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(result_ids, vec!["call-1", "call-2"]);

        let completed_ids = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionCompleted { tool_call_id, .. } => Some(tool_call_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(completed_ids, vec!["call-1", "call-2"]);
    }

    #[tokio::test]
    async fn cancellation_drops_in_flight_and_queued_tool_jobs() {
        let active_calls = Arc::new(AtomicUsize::new(0));
        let peak_calls = Arc::new(AtomicUsize::new(0));
        let tool = DelayedTool {
            // A limit of one means this first call waits forever unless it is
            // dropped; the second model call remains queued behind it.
            barrier: Arc::new(Barrier::new(2)),
            active_calls: active_calls.clone(),
            peak_calls: peak_calls.clone(),
        };
        let config = AgentConfig {
            max_concurrent_tool_calls: 1,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(
            Box::new(TwoTurnToolProvider::default()),
            test_model(),
            config,
        );
        agent.add_tool(Arc::new(tool));
        agent.add_user_message("run both calls");
        let cancellation = agent.cancellation();
        let run = tokio::spawn(async move {
            let result = agent.run().await;
            (agent, result)
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while active_calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first tool call should begin");
        cancellation.cancel();

        let (agent, result) = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("cancellation should release pending tool jobs")
            .unwrap();
        assert!(result.is_err());
        assert_eq!(agent.state(), &AgentState::Aborted);
        assert_eq!(peak_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_history_with_multiple_unmatched_tool_calls() {
        let model = test_model();
        let mut agent = Agent::new(Box::new(FauxProvider::new()), model, AgentConfig::default());

        let assistant = crate::ai::types::AssistantMessage::new(
            vec![
                AssistantContent::ToolCall(crate::ai::types::ToolCall::new(
                    "call-1",
                    "bash",
                    serde_json::json!({}),
                )),
                AssistantContent::ToolCall(crate::ai::types::ToolCall::new(
                    "call-2",
                    "bash",
                    serde_json::json!({}),
                )),
            ],
            crate::ai::types::Api::OpenaiCompletions,
            "test",
            "test",
            StopReason::ToolUse,
        );

        let result = agent.load_history(vec![
            Message::Assistant(assistant),
            Message::ToolResult(crate::ai::types::ToolResultMessage::new(
                "call-1", "bash", "ok", false,
            )),
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn caches_prompt_files_until_explicit_reload() {
        let directory =
            std::env::temp_dir().join(format!("hallucinate-context-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("AGENTS.md"), "Initial instructions.").unwrap();

        let mut agent = Agent::new(
            Box::new(FauxProvider::new()),
            test_model(),
            AgentConfig::default(),
        )
        .with_system_prompt("Base prompt.".to_string())
        .with_cwd(directory.to_string_lossy().into_owned());
        let first = agent.build_context();
        let first_prompt = first.system_prompt.as_ref().unwrap().clone();

        std::fs::write(directory.join("AGENTS.md"), "Changed instructions.").unwrap();
        let second = agent.build_context();

        assert!(
            second
                .system_prompt
                .as_ref()
                .unwrap()
                .contains("Initial instructions.")
        );
        assert!(Arc::ptr_eq(
            first.system_prompt.as_ref().unwrap(),
            second.system_prompt.as_ref().unwrap()
        ));

        agent.reload_system_prompt();
        let refreshed = agent.build_context();
        assert_ne!(refreshed.system_prompt.as_ref().unwrap(), &first_prompt);
        assert!(
            refreshed
                .system_prompt
                .as_ref()
                .unwrap()
                .contains("Changed instructions.")
        );

        let alternate_directory = directory.with_file_name(format!(
            "hallucinate-context-cache-other-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&alternate_directory);
        std::fs::create_dir_all(&alternate_directory).unwrap();
        std::fs::write(alternate_directory.join("AGENTS.md"), "Other instructions.").unwrap();
        agent.set_cwd(alternate_directory.to_string_lossy().into_owned());
        let changed_cwd = agent.build_context();
        assert!(
            changed_cwd
                .system_prompt
                .as_ref()
                .unwrap()
                .contains("Other instructions.")
        );

        let _ = std::fs::remove_dir_all(directory);
        let _ = std::fs::remove_dir_all(alternate_directory);
    }

    #[test]
    fn manual_compaction_reduces_history_before_context_limit() {
        let mut agent = Agent::new(
            Box::new(FauxProvider::new()),
            test_model(),
            AgentConfig::default(),
        );
        agent.add_user_message("first ".repeat(200));
        agent.add_user_message("second ".repeat(200));
        agent.add_user_message("latest ".repeat(200));

        let tokens_before = agent.estimate_tokens();
        agent.compact().unwrap();

        assert!(agent.estimate_tokens() < tokens_before);
        assert!(agent.messages().iter().any(|message| {
            matches!(message, Message::User(message) if message.content.starts_with("[Context Summary]"))
        }));
    }

    #[test]
    fn cached_token_total_matches_a_full_context_recomputation() {
        let config = AgentConfig {
            max_context_tokens: 1_000,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(Box::new(FauxProvider::new()), test_model(), config)
            .with_system_prompt("Base prompt.".to_string());
        agent.add_user_message("first message");
        agent.add_assistant_message(AssistantMessage::new(
            vec![AssistantContent::Text(crate::ai::types::TextContent::new(
                "second message",
            ))],
            Api::OpenaiCompletions,
            "test",
            "test-model",
            StopReason::Stop,
        ));
        agent.add_user_message("x".repeat(8_000));

        let context = agent.build_context();
        assert_eq!(
            agent.message_token_total + agent.context_base().static_token_cost,
            crate::agent::types::estimate_context_tokens(&context)
        );

        agent.compact().unwrap();
        let compacted_context = agent.build_context();
        assert_eq!(
            agent.message_token_total + agent.context_base().static_token_cost,
            crate::agent::types::estimate_context_tokens(&compacted_context)
        );
    }

    #[test]
    fn excludes_tool_definitions_when_disabled() {
        let config = AgentConfig {
            include_tool_definitions: false,
            ..AgentConfig::default()
        };

        let mut agent = Agent::with_all_tools_and_image_resize(
            Box::new(FauxProvider::new()),
            test_model(),
            config,
            true,
        );

        assert!(agent.build_context().tools.is_none());
    }

    #[test]
    fn advertises_skills_when_the_read_tool_is_enabled() {
        let mut agent = Agent::with_all_tools_and_image_resize(
            Box::new(FauxProvider::new()),
            test_model(),
            AgentConfig::default(),
            true,
        )
        .with_skills(vec![Skill {
            name: "release-helper".to_string(),
            description: "Prepare release notes".to_string(),
            file_path: PathBuf::from("/tmp/release/SKILL.md"),
            base_dir: PathBuf::from("/tmp/release"),
            disable_model_invocation: false,
        }]);

        let prompt = agent.build_context().system_prompt.unwrap();

        assert!(prompt.contains("<name>release-helper</name>"));
        assert!(prompt.contains("/tmp/release/SKILL.md"));
    }
}
