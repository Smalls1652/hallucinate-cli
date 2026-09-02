//! Synchronize agent lifecycle events and persisted messages with the UI.

use hallucinate_agent::{
    agent::types::{AgentEvent, AgentState},
    ai::types::{AssistantContent, AssistantMessage, Message},
};

use super::InteractiveTui;
use crate::alt_screen::{
    Message as TuiMessage,
    MessageRole,
    ToolCall as TuiToolCall,
    TranscriptRenderer,
};

impl InteractiveTui {
    /// Process any pending agent events.
    pub(super) fn process_events(&mut self) -> bool {
        let mut received_delta = false;

        while let Ok(event) = self.event_rx.try_recv() {
            self.render_state.status = true;
            self.render_state.transcript = true;

            if let Err(error) = self.session_manager.save_agent_event(&event) {
                eprintln!("Warning: could not persist session event: {error}");
            }

            match &event {
                AgentEvent::StateChanged { new_state, .. } => {
                    self.agent_state = new_state.clone();

                    match new_state {
                        AgentState::Streaming => {
                            // Text and reasoning are displayed as their delta
                            // events arrive.
                        },
                        AgentState::ToolExecuting => {
                            // Tool is being executed
                        },
                        AgentState::TurnComplete => {
                            // Response complete, ready for next input
                        },
                        AgentState::Error(msg) => {
                            self.show_notice(format!("❌ Error: {}", msg));
                        },
                        AgentState::Aborted => {
                            self.show_notice("Agent aborted.");
                        },
                        _ => {},
                    }
                },

                AgentEvent::AssistantTextDelta { delta, .. } => {
                    let index = self.ensure_streaming_assistant_message();
                    self.transcript.append_to_message(index, delta);
                    received_delta = true;
                },

                AgentEvent::AssistantThinkingDelta { delta } => {
                    let index = self.ensure_streaming_assistant_message();
                    self.transcript.append_thinking_to_message(index, delta);
                    received_delta = true;
                },

                AgentEvent::AssistantMessageCompleted { message, .. } => {
                    let (content, thinking, tool_calls) = display_assistant_message(message);

                    if let Some(index) = self.streaming_message_index.take() {
                        self.transcript
                            .complete_message(index, content, thinking, tool_calls);
                    } else {
                        self.transcript.add_message(TuiMessage {
                            role: MessageRole::Assistant,
                            content,
                            thinking,
                            tool_calls,
                            timestamp: chrono::Utc::now(),
                        });
                    }
                },

                AgentEvent::ToolCallDetected { .. } | AgentEvent::ToolExecutionStarted { .. } => {},

                AgentEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    is_error,
                    ..
                } => {
                    self.transcript
                        .complete_tool_call(tool_call_id, result.clone(), *is_error);
                },

                AgentEvent::ToolExecutionFailed {
                    tool_call_id,
                    error,
                    ..
                } => {
                    self.transcript
                        .complete_tool_call(tool_call_id, error.clone(), true);
                },

                AgentEvent::TurnCompleted { .. } => {
                    self.session_manager.touch();
                },

                _ => {},
            }
        }

        received_delta
    }

    /// Create (once) the assistant card receiving streamed response data.
    fn ensure_streaming_assistant_message(&mut self) -> usize {
        if let Some(index) = self.streaming_message_index {
            return index;
        }

        self.transcript.add_message(TuiMessage {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });

        let index = self.transcript.messages().len() - 1;
        self.streaming_message_index = Some(index);
        index
    }

    /// Restore completed agents and display their responses without blocking
    /// terminal input or rendering.
    pub(super) fn process_completed_runs(&mut self) -> bool {
        let mut completed_run = false;

        while let Ok(completion) = self.agent_completion_rx.try_recv() {
            completed_run = true;
            self.token_count = completion.agent.estimate_tokens();
            self.agent_cancellation = None;
            self.agent = Some(completion.agent);

            let agent = self.agent.as_ref().expect("completed agent is restored");
            self.agent_state = agent.state().clone();

            // The user message is written before the background task starts.
            // Persist every assistant and tool message which is not already in
            // the archive, including messages produced after compaction. A
            // count-based index is not safe here because `/compact` replaces
            // the in-memory history while deliberately retaining the complete
            // archived session.
            let mut persisted_messages = self
                .session_manager
                .current_storage()
                .and_then(|storage| match storage.load_messages() {
                    Ok(messages) => Some(messages),
                    Err(error) => {
                        eprintln!("Warning: could not inspect persisted messages: {error}");
                        None
                    },
                })
                .unwrap_or_default();

            for message in agent.messages() {
                if matches!(message, Message::User(_)) {
                    continue;
                }

                let already_persisted = persisted_messages.iter().any(|persisted| {
                    serde_json::to_string(persisted).ok() == serde_json::to_string(message).ok()
                });

                if already_persisted {
                    continue;
                }

                if let Err(error) = self.session_manager.save_message(message) {
                    eprintln!("Warning: could not persist session message: {error}");
                } else {
                    persisted_messages.push(message.clone());
                }
            }

            if let Err(error) = completion.result
                && !matches!(self.agent_state, AgentState::Aborted | AgentState::Error(_))
            {
                // The matching Error/Aborted event has already been consumed
                // by process_events; the status bar is the single visible
                // error presentation here.
                self.agent_state = AgentState::Error(error);
            }
        }

        completed_run
    }
}

/// Rebuild the visible transcript from persisted conversation history.
pub(super) fn transcript_from_messages(messages: &[Message]) -> TranscriptRenderer {
    let mut transcript = TranscriptRenderer::new();

    for message in messages {
        match message {
            Message::User(message) => transcript.add_message(TuiMessage {
                role: MessageRole::User,
                content: message.content.clone(),
                thinking: String::new(),
                tool_calls: Vec::new(),
                timestamp: timestamp_from_millis(message.timestamp),
            }),

            Message::Assistant(message) => {
                let (content, thinking, tool_calls) = display_assistant_message(message);
                transcript.add_message(TuiMessage {
                    role: MessageRole::Assistant,
                    content,
                    thinking,
                    tool_calls,
                    timestamp: timestamp_from_millis(message.timestamp),
                });
            },

            Message::ToolResult(message) => {
                let content = message
                    .content
                    .iter()
                    .map(|item| item.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

                if !transcript.complete_tool_call(
                    &message.tool_call_id,
                    content.clone(),
                    message.is_error,
                ) {
                    transcript.add_message(TuiMessage {
                        role: MessageRole::Tool,
                        content: format!("{}: {}", message.tool_name, content),
                        thinking: String::new(),
                        tool_calls: Vec::new(),
                        timestamp: timestamp_from_millis(message.timestamp),
                    });
                }
            },
        }
    }

    transcript
}

/// Convert an agent response into the transcript representation.
fn display_assistant_message(message: &AssistantMessage) -> (String, String, Vec<TuiToolCall>) {
    let tool_calls = message
        .content
        .iter()
        .filter_map(|content| {
            if let AssistantContent::ToolCall(tool_call) = content {
                Some(TuiToolCall {
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                    result: None,
                })
            } else {
                None
            }
        })
        .collect();

    let text = message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();

    let thinking = message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Thinking(thinking) => Some(thinking.thinking.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    (text, thinking, tool_calls)
}

/// Convert a Unix timestamp in milliseconds to a UTC datetime.
fn timestamp_from_millis(timestamp: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(timestamp as i64).unwrap_or_else(chrono::Utc::now)
}

#[cfg(test)]
mod tests {
    use hallucinate_agent::ai::types::{Api, StopReason, ToolCall, ToolResultMessage};

    use super::*;

    #[test]
    fn persisted_tool_results_reconstruct_the_original_transcript_call() {
        let messages = vec![
            Message::User(hallucinate_agent::ai::types::UserMessage::new("run it")),
            Message::Assistant(AssistantMessage::new(
                vec![AssistantContent::ToolCall(ToolCall::new(
                    "call-1",
                    "bash",
                    serde_json::json!({ "command": "printf ok" }),
                ))],
                Api::OpenaiCompletions,
                "openai",
                "test",
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::new("call-1", "bash", "ok", false)),
        ];

        let transcript = transcript_from_messages(&messages);

        assert_eq!(transcript.messages().len(), 2);
        assert_eq!(transcript.messages()[0].role, MessageRole::User);
        assert_eq!(
            transcript.messages()[1].tool_calls[0].result.as_deref(),
            Some("✅ ok")
        );
    }

    #[test]
    fn assistant_display_keeps_thinking_separate_from_answer_text() {
        let message = AssistantMessage::new(
            vec![
                AssistantContent::Thinking(hallucinate_agent::ai::types::ThinkingContent::new(
                    "plan",
                )),
                AssistantContent::Text(hallucinate_agent::ai::types::TextContent::new("answer")),
            ],
            Api::OpenaiCompletions,
            "test",
            "model",
            StopReason::Stop,
        );

        let (content, thinking, tools) = display_assistant_message(&message);

        assert_eq!(content, "answer");
        assert_eq!(thinking, "plan");
        assert!(tools.is_empty());
    }
}
