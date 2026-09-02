//! Context-history selection and serialization for LLM-backed compaction.
//!
//! The runtime owns the summary request. This module selects a safe retained
//! suffix and renders the omitted provider messages for that request.

use crate::ai::types::{AssistantContent, Message};

/// Select an older prefix to summarize and a recent suffix to retain.
///
/// The retained suffix always starts at a user message. This deliberately
/// preserves complete user turns and prevents an assistant tool call from
/// being separated from its required following tool results.
pub fn split_messages_at_turn_boundary(
    messages: &[Message],
    costs: &[u64],
    keep_recent_tokens: u64,
) -> Result<(Vec<Message>, Vec<Message>), anyhow::Error> {
    if messages.len() != costs.len() {
        return Err(anyhow::anyhow!(
            "message and token-cost counts differ during context compaction"
        ));
    }

    let mut suffix_tokens = 0_u64;
    let mut cut_index = messages.len();

    for index in (0..messages.len()).rev() {
        suffix_tokens = suffix_tokens.saturating_add(costs[index]);

        if suffix_tokens >= keep_recent_tokens && matches!(messages[index], Message::User(_)) {
            cut_index = index;
            break;
        }
    }

    // A conversation shorter than the requested retained tail cannot be
    // compacted without splitting its only turn.
    if cut_index == 0 || cut_index == messages.len() {
        return Ok((messages.to_vec(), Vec::new()));
    }

    Ok((
        messages[..cut_index].to_vec(),
        messages[cut_index..].to_vec(),
    ))
}

/// Render messages as data inside an LLM summarization request.
///
/// Role labels and explicit tool blocks prevent the summarizer from treating
/// the archived transcript as instructions to follow.
pub fn serialize_messages(messages: &[Message]) -> String {
    messages
        .iter()
        .map(serialize_message)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn serialize_message(message: &Message) -> String {
    match message {
        Message::User(message) => format!("<user>\n{}\n</user>", message.content),
        Message::Assistant(message) => {
            let content = message
                .content
                .iter()
                .map(|content| match content {
                    AssistantContent::Text(text) => text.text.clone(),
                    AssistantContent::Thinking(thinking) => {
                        format!("<thinking>\n{}\n</thinking>", thinking.thinking)
                    },
                    AssistantContent::ToolCall(call) => format!(
                        "<tool-call name=\"{}\" id=\"{}\">\n{}\n</tool-call>",
                        call.name, call.id, call.arguments
                    ),
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!("<assistant>\n{content}\n</assistant>")
        },
        Message::ToolResult(message) => format!(
            "<tool-result name=\"{}\" id=\"{}\">\n{}\n</tool-result>",
            message.tool_name,
            message.tool_call_id,
            message
                .content
                .iter()
                .map(|content| content.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{
        Api,
        AssistantContent,
        AssistantMessage,
        StopReason,
        ToolCall,
        ToolResultMessage,
        UserMessage,
    };

    #[test]
    fn retains_complete_turns_at_user_message_boundaries() {
        let messages = vec![
            Message::User(UserMessage::new("first")),
            Message::User(UserMessage::new("second")),
            Message::User(UserMessage::new("latest")),
        ];
        let (omitted, retained) =
            split_messages_at_turn_boundary(&messages, &[10, 10, 10], 15).unwrap();

        assert_eq!(omitted.len(), 1);
        assert_eq!(retained.len(), 2);
        assert!(matches!(&retained[0], Message::User(message) if message.content == "second"));
    }

    #[test]
    fn never_separates_tool_calls_from_their_results() {
        let messages = vec![
            Message::User(UserMessage::new("first")),
            Message::Assistant(AssistantMessage::new(
                vec![AssistantContent::ToolCall(ToolCall::new(
                    "call-1",
                    "read",
                    serde_json::json!({ "path": "Cargo.toml" }),
                ))],
                Api::OpenaiCompletions,
                "test",
                "test",
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::new("call-1", "read", "contents", false)),
            Message::User(UserMessage::new("second")),
        ];
        let (omitted, retained) =
            split_messages_at_turn_boundary(&messages, &[10, 10, 10, 10], 10).unwrap();

        assert_eq!(omitted.len(), 3);
        assert!(
            matches!(retained.as_slice(), [Message::User(message)] if message.content == "second")
        );
    }
}
