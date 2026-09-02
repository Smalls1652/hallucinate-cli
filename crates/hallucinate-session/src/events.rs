//! Durable archival policy for agent lifecycle events.
//!
//! Streaming deltas and completed assistant messages are intentionally omitted:
//! completed messages are persisted through the session message API instead.

use hallucinate_agent::agent::types::AgentEvent;

/// Convert an agent lifecycle event into its stable session record.
pub(crate) fn archive_event(event: &AgentEvent) -> Option<(&'static str, serde_json::Value)> {
    match event {
        AgentEvent::StateChanged {
            new_state,
            old_state,
        } => Some((
            "state_changed",
            serde_json::json!({ "old_state": old_state.to_string(), "new_state": new_state.to_string() }),
        )),
        AgentEvent::TurnStarted { turn_number } => Some((
            "turn_started",
            serde_json::json!({ "turn_number": turn_number }),
        )),
        AgentEvent::ToolCallDetected {
            tool_call_id,
            tool_name,
            arguments,
        } => Some((
            "tool_call_detected",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "arguments": arguments }),
        )),
        AgentEvent::ToolExecutionStarted {
            tool_call_id,
            tool_name,
        } => Some((
            "tool_execution_started",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name }),
        )),
        AgentEvent::ToolExecutionCompleted {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => Some((
            "tool_execution_completed",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "result": result, "is_error": is_error }),
        )),
        AgentEvent::ToolExecutionFailed {
            tool_call_id,
            tool_name,
            error,
        } => Some((
            "tool_execution_failed",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "error": error }),
        )),
        AgentEvent::TurnCompleted {
            turn_number,
            stop_reason,
        } => Some((
            "turn_completed",
            serde_json::json!({ "turn_number": turn_number, "stop_reason": stop_reason.to_string() }),
        )),
        AgentEvent::Error { message } => Some(("error", serde_json::json!({ "message": message }))),
        AgentEvent::CompactionPerformed {
            tokens_before,
            tokens_after,
            summaries_added,
        } => Some((
            "compaction",
            serde_json::json!({ "tokens_before": tokens_before, "tokens_after": tokens_after, "summaries_added": summaries_added }),
        )),
        AgentEvent::Aborted => Some(("aborted", serde_json::json!({}))),
        // Tool-call construction is high-frequency streamed presentation
        // state. The validated ToolCallDetected event records the durable
        // lifecycle entry once the provider has completed its JSON payload.
        AgentEvent::AssistantTextDelta { .. }
        | AgentEvent::AssistantThinkingDelta { .. }
        | AgentEvent::AssistantMessageCompleted { .. }
        | AgentEvent::ToolCallStarted { .. }
        | AgentEvent::ToolCallDelta { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use hallucinate_agent::agent::types::{AgentEvent, AgentState};

    use super::*;

    #[test]
    fn archives_lifecycle_events_and_omits_message_content() {
        let events = [
            AgentEvent::StateChanged {
                old_state: AgentState::Idle,
                new_state: AgentState::Streaming,
            },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::ToolCallDetected {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "true" }),
            },
            AgentEvent::ToolExecutionStarted {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
            },
            AgentEvent::ToolExecutionCompleted {
                tool_call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                result: "ok".to_string(),
                is_error: false,
            },
            AgentEvent::ToolExecutionFailed {
                tool_call_id: "call-2".to_string(),
                tool_name: "read".to_string(),
                error: "missing".to_string(),
            },
            AgentEvent::TurnCompleted {
                turn_number: 1,
                stop_reason: hallucinate_agent::ai::types::StopReason::Stop,
            },
            AgentEvent::Error {
                message: "failed".to_string(),
            },
            AgentEvent::CompactionPerformed {
                tokens_before: 100,
                tokens_after: 50,
                summaries_added: 1,
            },
            AgentEvent::Aborted,
        ];

        let names = events
            .iter()
            .filter_map(archive_event)
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), events.len());
        assert_eq!(
            archive_event(&AgentEvent::AssistantTextDelta {
                delta: "x".to_string()
            }),
            None
        );
        assert_eq!(
            archive_event(&AgentEvent::AssistantThinkingDelta {
                delta: "x".to_string()
            }),
            None
        );
        assert_eq!(
            archive_event(&AgentEvent::AssistantMessageCompleted {
                turn_number: 1,
                message: Box::new(hallucinate_agent::ai::types::AssistantMessage::new(
                    Vec::new(),
                    hallucinate_agent::ai::types::Api::OpenaiCompletions,
                    "openai",
                    "test",
                    hallucinate_agent::ai::types::StopReason::Stop,
                )),
            }),
            None
        );
    }
}
