//! Event stream protocol for assistant message generation.
//!
//! Streams emit events as the LLM generates content, terminating with either
//! a `done` event (successful completion) or an `error` event (failure).

use std::{
    collections::HashMap,
    fmt,
    pin::Pin,
    sync::{Arc, Mutex},
};

use futures::stream::Stream;
use tokio::sync::mpsc;

use crate::ai::types::{
    AssistantContent, AssistantMessage, StopReason, TextContent, ThinkingContent, ToolCall,
};

// ── Event Types ──────────────────────────────────────────────────────────────

/// Events emitted during assistant message generation.
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    /// Stream started.
    Start,

    /// Text content block started.
    TextStart { content_index: usize },

    /// Text content delta.
    TextDelta { content_index: usize, delta: String },

    /// Text content block completed.
    TextEnd {
        content_index: usize,
        content: String,
    },

    /// Thinking/reasoning block started.
    ThinkingStart { content_index: usize },

    /// Thinking/reasoning delta.
    ThinkingDelta { content_index: usize, delta: String },

    /// Thinking/reasoning block completed.
    ThinkingEnd {
        content_index: usize,
        content: String,
    },

    /// Tool call started.
    ToolcallStart { content_index: usize },

    /// Tool call argument delta.
    ToolcallDelta { content_index: usize, delta: String },

    /// Tool call completed.
    ToolcallEnd {
        content_index: usize,
        tool_call: ToolCall,
    },

    /// Stream completed successfully.
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },

    /// Stream terminated with an error.
    Error {
        reason: StopReason,
        error: AssistantMessage,
    },
}

impl fmt::Display for AssistantMessageEvent {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        match self {
            AssistantMessageEvent::Start => write!(f, "[start]"),

            AssistantMessageEvent::TextStart { content_index, .. } => {
                write!(f, "[text_start idx={content_index}]")
            },

            AssistantMessageEvent::TextDelta { delta, .. } => {
                write!(f, "[text_delta len={}]", delta.len())
            },

            AssistantMessageEvent::TextEnd {
                content_index,
                content,
                ..
            } => {
                write!(f, "[text_end idx={content_index} len={}]", content.len())
            },

            AssistantMessageEvent::ThinkingStart { content_index, .. } => {
                write!(f, "[thinking_start idx={content_index}]")
            },

            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                write!(f, "[thinking_delta len={}]", delta.len())
            },

            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
                ..
            } => {
                write!(
                    f,
                    "[thinking_end idx={content_index} len={}]",
                    content.len()
                )
            },

            AssistantMessageEvent::ToolcallStart { content_index, .. } => {
                write!(f, "[toolcall_start idx={content_index}]")
            },

            AssistantMessageEvent::ToolcallDelta { delta, .. } => {
                write!(f, "[toolcall_delta len={}]", delta.len())
            },

            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
                ..
            } => {
                write!(
                    f,
                    "[toolcall_end idx={content_index} name={}",
                    tool_call.name
                )
            },

            AssistantMessageEvent::Done { reason, .. } => write!(f, "[done reason={reason}]"),

            AssistantMessageEvent::Error { reason, .. } => write!(f, "[error reason={reason}]"),
        }
    }
}

// ── Event Stream ─────────────────────────────────────────────────────────────

/// An async stream of assistant message events.
///
/// Created via [`create_assistant_message_event_stream`]. Events are pushed
/// by the producer and consumed via async iteration or the [`Stream`] trait.
#[derive(Clone)]
pub struct AssistantMessageEventStream {
    /// Channel sender for pushing events into the stream.
    sender: mpsc::Sender<AssistantMessageEvent>,

    /// Arc-wrapped channel receiver for polling events.
    receiver: Arc<Mutex<mpsc::Receiver<AssistantMessageEvent>>>,

    /// Arc-wrapped mutex holding the final message after stream completion.
    pub final_message: Arc<Mutex<Option<AssistantMessage>>>,
}

impl AssistantMessageEventStream {
    /// Push an event into the stream.
    pub async fn push(
        &self,
        event: AssistantMessageEvent,
    ) -> Result<(), StreamError> {
        let is_terminal = matches!(
            event,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        );

        self.sender
            .send(event)
            .await
            .map_err(|_| StreamError::ChannelClosed)?;

        // The stream owns a sender so dropping producer tasks alone cannot
        // close the channel. Close the receiver after a terminal event so
        // consumers drain the queued events and then terminate.
        if is_terminal {
            self.receiver.lock().unwrap().close();
        }

        Ok(())
    }
}

impl Stream for AssistantMessageEventStream {
    type Item = AssistantMessageEvent;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut receiver = self.receiver.lock().unwrap();

        // `Receiver::close` still permits already-buffered events to be
        // consumed, including the terminal Done or Error event.
        Pin::new(&mut *receiver).poll_recv(cx)
    }
}

/// Error type for stream operations.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("stream channel closed")]
    ChannelClosed,
}

/// Create a new assistant message event stream.
///
/// Returns an [`AssistantMessageEventStream`] that can be used to push events
/// and iterate over them asynchronously.
pub fn create_assistant_message_event_stream() -> AssistantMessageEventStream {
    let (sender, receiver) = mpsc::channel::<AssistantMessageEvent>(64);

    AssistantMessageEventStream {
        sender,
        receiver: Arc::new(Mutex::new(receiver)),
        final_message: Arc::new(Mutex::new(None)),
    }
}

/// Builder for constructing assistant messages with event emission.
///
/// This helper accumulates content blocks and emits appropriate events
/// as content is added.
pub struct AssistantMessageBuilder {
    /// The accumulated assistant message being built.
    pub message: AssistantMessage,

    /// The event stream to emit events into.
    pub stream: AssistantMessageEventStream,

    /// Maps provider text delta indices to stable content-block indices.
    text_indices: HashMap<usize, usize>,

    /// Maps provider reasoning delta indices to stable content-block indices.
    thinking_indices: HashMap<usize, usize>,

    /// Maps provider tool call indices to stable content-block indices.
    tool_indices: HashMap<usize, usize>,

    /// Buffers raw argument JSON fragments per provider until completion.
    tool_arguments: HashMap<usize, String>,
}

impl AssistantMessageBuilder {
    /// Create a new builder with the given stream and model info.
    pub fn new(
        stream: AssistantMessageEventStream,
        api: crate::ai::types::Api,
        model: impl Into<String>,
    ) -> Self {
        Self {
            message: AssistantMessage::new(
                Vec::new(),
                api,
                "openai-compat",
                model,
                StopReason::Pending,
            ),
            stream,
            text_indices: HashMap::new(),
            thinking_indices: HashMap::new(),
            tool_indices: HashMap::new(),
            tool_arguments: HashMap::new(),
        }
    }

    /// Emit the initial [`Start`](AssistantMessageEvent::Start) event.
    pub async fn emit_start(&mut self) {
        let _ = self.stream.push(AssistantMessageEvent::Start).await;
    }

    /// Append a provider text delta. A provider index is mapped to a stable
    /// content-block index so incremental consumers can reconstruct the block
    /// from its deltas.
    pub async fn append_text(
        &mut self,
        provider_index: usize,
        delta: &str,
    ) {
        let index = if let Some(index) = self.text_indices.get(&provider_index) {
            *index
        } else {
            let index = self.message.content.len();

            self.message
                .content
                .push(AssistantContent::Text(TextContent::new("")));

            self.text_indices.insert(provider_index, index);

            let _ = self
                .stream
                .push(AssistantMessageEvent::TextStart {
                    content_index: index,
                })
                .await;

            index
        };

        if let AssistantContent::Text(text) = &mut self.message.content[index] {
            text.text.push_str(delta);
        }

        let _ = self
            .stream
            .push(AssistantMessageEvent::TextDelta {
                content_index: index,
                delta: delta.to_string(),
            })
            .await;
    }

    /// Append a provider reasoning delta. Reasoning is tracked separately from
    /// ordinary text because providers may interleave both block types.
    pub async fn append_thinking(
        &mut self,
        provider_index: usize,
        delta: &str,
    ) {
        let index = if let Some(index) = self.thinking_indices.get(&provider_index) {
            *index
        } else {
            let index = self.message.content.len();

            self.message
                .content
                .push(AssistantContent::Thinking(ThinkingContent::new("")));

            self.thinking_indices.insert(provider_index, index);

            let _ = self
                .stream
                .push(AssistantMessageEvent::ThinkingStart {
                    content_index: index,
                })
                .await;

            index
        };

        if let AssistantContent::Thinking(thinking) = &mut self.message.content[index] {
            thinking.thinking.push_str(delta);
        }

        let _ = self
            .stream
            .push(AssistantMessageEvent::ThinkingDelta {
                content_index: index,
                delta: delta.to_string(),
            })
            .await;
    }

    /// Record one OpenAI tool-call delta. OpenAI sends id/name/arguments over
    /// separate chunks; argument JSON is validated only when the call closes.
    pub async fn append_tool_call(
        &mut self,
        provider_index: usize,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        let index = if let Some(index) = self.tool_indices.get(&provider_index) {
            *index
        } else {
            let index = self.message.content.len();

            self.message
                .content
                .push(AssistantContent::ToolCall(ToolCall::new(
                    id.unwrap_or_default(),
                    name.unwrap_or_default(),
                    serde_json::Value::Null,
                )));

            self.tool_indices.insert(provider_index, index);
            self.tool_arguments.insert(provider_index, String::new());

            let _ = self
                .stream
                .push(AssistantMessageEvent::ToolcallStart {
                    content_index: index,
                })
                .await;

            index
        };

        if let AssistantContent::ToolCall(call) = &mut self.message.content[index] {
            if let Some(id) = id.filter(|value| !value.is_empty()) {
                call.id = id.to_string();
            }

            if let Some(name) = name.filter(|value| !value.is_empty()) {
                call.name = name.to_string();
            }
        }

        if let Some(delta) = arguments {
            let argument_buffer = self.tool_arguments.entry(provider_index).or_default();
            argument_buffer.push_str(delta);

            // Incomplete JSON cannot be represented as an object yet, so keep
            // the raw argument buffer in the accumulated tool call. `finish`
            // replaces it with the validated JSON value before ToolcallEnd.
            if let AssistantContent::ToolCall(call) = &mut self.message.content[index] {
                call.arguments = serde_json::Value::String(argument_buffer.clone());
            }

            let _ = self
                .stream
                .push(AssistantMessageEvent::ToolcallDelta {
                    content_index: index,
                    delta: delta.to_string(),
                })
                .await;
        }
    }

    /// Set the token usage information on the built message.
    pub fn set_usage(
        &mut self,
        usage: crate::ai::types::Usage,
    ) {
        self.message.usage = usage;
    }

    /// Close all open content blocks and emit their terminal events.
    async fn close_content_blocks(&mut self) -> Result<(), String> {
        let mut text_blocks: Vec<_> = self.text_indices.values().copied().collect();
        text_blocks.sort_unstable();
        text_blocks.dedup();

        for index in text_blocks {
            let content = self.message.content[index]
                .text_content()
                .unwrap_or_default()
                .to_string();

            let _ = self
                .stream
                .push(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content,
                })
                .await;
        }

        let mut thinking_blocks: Vec<_> = self.thinking_indices.values().copied().collect();
        thinking_blocks.sort_unstable();
        thinking_blocks.dedup();

        for index in thinking_blocks {
            let content = match &self.message.content[index] {
                AssistantContent::Thinking(thinking) => thinking.thinking.clone(),
                _ => unreachable!("thinking index must reference a thinking block"),
            };

            let _ = self
                .stream
                .push(AssistantMessageEvent::ThinkingEnd {
                    content_index: index,
                    content,
                })
                .await;
        }

        let mut tools: Vec<_> = self
            .tool_indices
            .iter()
            .map(|(provider, index)| (*provider, *index))
            .collect();
        tools.sort_unstable_by_key(|(provider, _)| *provider);

        for (provider_index, index) in tools {
            let arguments = self
                .tool_arguments
                .remove(&provider_index)
                .unwrap_or_default();

            let parsed = serde_json::from_str(&arguments).map_err(|error| {
                format!("invalid JSON arguments for streamed tool call: {error}")
            })?;

            let tool_call = match &mut self.message.content[index] {
                AssistantContent::ToolCall(call) => {
                    if call.id.is_empty() || call.name.is_empty() {
                        return Err(
                            "streamed tool call is missing an id or function name".to_string()
                        );
                    }

                    call.arguments = parsed;
                    call.clone()
                },
                _ => unreachable!("tool index must reference a tool call"),
            };

            let _ = self
                .stream
                .push(AssistantMessageEvent::ToolcallEnd {
                    content_index: index,
                    tool_call,
                })
                .await;
        }

        Ok(())
    }

    /// Complete all open blocks and emit the final accumulated message.
    pub async fn finish(
        &mut self,
        reason: StopReason,
    ) -> Result<(), String> {
        self.close_content_blocks().await?;

        let message = AssistantMessage {
            stop_reason: reason.clone(),
            ..self.message.clone()
        };

        *self.stream.final_message.lock().unwrap() = Some(message.clone());

        let _ = self
            .stream
            .push(AssistantMessageEvent::Done { reason, message })
            .await;

        Ok(())
    }

    /// Terminate the stream with an error event.
    pub async fn fail(
        &mut self,
        error: impl Into<String>,
    ) {
        let message = AssistantMessage {
            stop_reason: StopReason::Error,
            error_message: Some(error.into()),
            ..self.message.clone()
        };

        *self.stream.final_message.lock().unwrap() = Some(message.clone());

        let _ = self
            .stream
            .push(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error: message,
            })
            .await;
    }
}

// ── Helper Extension ─────────────────────────────────────────────────────────

impl AssistantContent {
    /// Return the text content if this is a text block, otherwise `None`.
    pub fn text_content(&self) -> Option<&str> {
        match self {
            AssistantContent::Text(t) => Some(&t.text),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::ai::types::Api;

    #[tokio::test]
    async fn test_stream_creation() {
        let _stream = create_assistant_message_event_stream();
    }

    #[tokio::test]
    async fn tool_arguments_accumulate_raw_fragments_until_completion() {
        let stream = create_assistant_message_event_stream();

        let mut builder =
            AssistantMessageBuilder::new(stream.clone(), Api::OpenaiCompletions, "test-model");

        builder
            .append_tool_call(0, Some("call-1"), Some("bash"), Some(r#"{"command":"#))
            .await;

        assert_eq!(
            builder.message.content[0],
            AssistantContent::ToolCall(ToolCall {
                type_: "toolCall".to_string(),
                id: "call-1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::Value::String(r#"{"command":"#.to_string()),
                thought_signature: None,
                namespace: None
            })
        );

        let argument_fragment = "\"hello\"}";
        builder
            .append_tool_call(0, None, None, Some(argument_fragment))
            .await;
        builder.finish(StopReason::ToolUse).await.unwrap();

        let events: Vec<_> = stream.collect().await;
        assert!(events.iter().any(|event| {
            matches!(event, AssistantMessageEvent::ToolcallEnd { tool_call, .. }
                if tool_call.arguments == serde_json::json!({ "command": "hello" }))
        }));
    }

    #[tokio::test]
    async fn incremental_events_only_contain_deltas_and_terminal_event_has_message() {
        let stream = create_assistant_message_event_stream();
        let mut builder =
            AssistantMessageBuilder::new(stream.clone(), Api::OpenaiCompletions, "test-model");

        builder.emit_start().await;
        for _ in 0..20 {
            builder.append_text(0, "long incremental response ").await;
            builder.append_thinking(0, "reasoning ").await;
        }
        builder
            .append_tool_call(
                0,
                Some("call-1"),
                Some("lookup"),
                Some(r#"{"query":"test"}"#),
            )
            .await;
        builder.finish(StopReason::ToolUse).await.unwrap();

        let events: Vec<_> = stream.collect().await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantMessageEvent::TextDelta { .. }))
                .count(),
            20
        );

        // The event type intentionally exposes the full AssistantMessage only
        // from Done/Error; all preceding events carry an index plus their
        // local delta or completed block value.
        let terminal_message = events
            .into_iter()
            .find_map(|event| match event {
                AssistantMessageEvent::Done { message, .. } => Some(message),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            terminal_message.content[0].text_content().unwrap().len(),
            520
        );
    }

    #[tokio::test]
    async fn test_push_and_iterate() {
        let stream = create_assistant_message_event_stream();

        // Push a simple done event
        let message = AssistantMessage::new(
            vec![AssistantContent::Text(TextContent::new("hello"))],
            Api::OpenaiCompletions,
            "test",
            "test-model",
            StopReason::Stop,
        );

        let _ = stream
            .push(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message: message.clone(),
            })
            .await;

        // Receive the event one at a time
        let event = futures::StreamExt::next(&mut stream.clone()).await;
        assert!(event.is_some());
        assert!(matches!(
            &event.unwrap(),
            AssistantMessageEvent::Done { .. }
        ));
        assert!(
            futures::StreamExt::next(&mut stream.clone())
                .await
                .is_none()
        );
    }
}
