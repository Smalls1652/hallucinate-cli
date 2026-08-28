//! Deterministic context compaction.
//!
//! The compactor returns *only* retained history plus summary text. Its caller
//! owns inserting summary messages, which makes duplicate insertion impossible.

use crate::{agent::types::estimate_message_tokens, ai::types::Message};

/// Compact history to approximately `max_tokens` while preserving chronological
/// order. Recent messages are retained; every omitted message is represented in
/// the returned summary text.
pub fn compact_messages(
    messages: &[Message],
    max_tokens: u32,
) -> Result<(Vec<Message>, Vec<String>), anyhow::Error> {
    let total: u64 = messages.iter().map(estimate_message_tokens).sum();
    if total <= u64::from(max_tokens) {
        return Ok((messages.to_vec(), Vec::new()));
    }
    if max_tokens < 32 {
        return Err(anyhow::anyhow!(
            "context budget is too small to compact safely"
        ));
    }

    // Reserve one third of the message budget for a deterministic summary.
    // There is intentionally no protected-message exception: newest messages
    // are retained by cost, and all omitted messages are summarized. Never
    // retain an oversized message just because it is the newest one: the
    // result must fit the requested budget even for a very large tool output.
    let summary_budget = u64::from(max_tokens / 3).max(16);
    let retained_budget = u64::from(max_tokens).saturating_sub(summary_budget);
    let costs: Vec<u64> = messages.iter().map(estimate_message_tokens).collect();
    let mut retained_indices = Vec::new();
    let mut used = 0_u64;

    for index in (0..messages.len()).rev() {
        let cost = costs[index];
        if cost <= retained_budget.saturating_sub(used) {
            retained_indices.push(index);
            used += cost;
        }
    }
    retained_indices.sort_unstable();
    let retained_set: std::collections::HashSet<_> = retained_indices.iter().copied().collect();
    let omitted: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| !retained_set.contains(index))
        .map(|(_, message)| message)
        .collect();

    if omitted.is_empty() {
        return Ok((messages.to_vec(), Vec::new()));
    }

    let summary = summarize(
        &omitted,
        (summary_budget.saturating_sub(1) as usize).saturating_mul(4),
    );
    let retained = retained_indices
        .into_iter()
        .map(|index| messages[index].clone())
        .collect();
    Ok((retained, vec![summary]))
}

fn summarize(
    messages: &[&Message],
    max_chars: usize,
) -> String {
    let mut lines = vec![format!("[{} earlier messages summarized]", messages.len())];
    let mut remaining = max_chars.saturating_sub(lines[0].len() + 1);
    for message in messages {
        if remaining < 12 {
            break;
        }
        let (role, content) = match message {
            Message::User(message) => ("User", message.content.clone()),
            Message::Assistant(message) => (
                "Assistant",
                message
                    .content
                    .iter()
                    .filter_map(|block| block.text_content())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Message::ToolResult(message) => (
                "Tool result",
                message
                    .content
                    .first()
                    .map(|item| item.text.clone())
                    .unwrap_or_default(),
            ),
        };
        let prefix = format!("{role}: ");
        let limit = remaining.saturating_sub(prefix.len()).min(200);
        let line = format!("{prefix}{}", truncate_string(&content, limit));
        remaining = remaining.saturating_sub(line.len() + 1);
        lines.push(line);
    }
    lines.join("\n")
}

fn truncate_string(
    s: &str,
    max_len: usize,
) -> String {
    let count = s.chars().count();
    if count <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return "...".chars().take(max_len).collect();
    }
    format!("{}...", s.chars().take(max_len - 3).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{Api, AssistantContent, AssistantMessage, StopReason, TextContent};

    fn user(content: &str) -> Message {
        Message::User(crate::ai::types::UserMessage::new(content))
    }
    fn assistant(content: &str) -> Message {
        Message::Assistant(AssistantMessage::new(
            vec![AssistantContent::Text(TextContent::new(content))],
            Api::OpenaiCompletions,
            "test",
            "test",
            StopReason::Stop,
        ))
    }

    #[test]
    fn no_compaction_when_within_budget() {
        let messages = vec![user("hello"), assistant("hi")];
        assert_eq!(compact_messages(&messages, 10_000).unwrap().0.len(), 2);
    }

    #[test]
    fn compaction_is_chronological_and_summarizes_every_omission() {
        let messages = vec![
            user("first unique"),
            assistant("second unique"),
            user(&"x".repeat(2000)),
            assistant("latest"),
        ];
        let (retained, summaries) = compact_messages(&messages, 200).unwrap();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].contains("User: x"));
        assert_eq!(retained.len(), 3);
        assert!(
            matches!(&retained[0], Message::User(message) if message.content == "first unique")
        );
        assert!(
            matches!(&retained[1], Message::Assistant(message) if message.content[0].text_content() == Some("second unique"))
        );
        assert!(
            matches!(retained.last(), Some(Message::Assistant(message)) if message.content[0].text_content() == Some("latest"))
        );
    }

    #[test]
    fn compacted_message_history_fits_the_message_budget() {
        let messages = vec![
            user(&"old ".repeat(500)),
            assistant(&"middle ".repeat(500)),
            user("latest"),
        ];
        let (retained, summaries) = compact_messages(&messages, 200).unwrap();
        let summary_message = Message::User(crate::ai::types::UserMessage::new(
            summaries.into_iter().next().unwrap(),
        ));
        let total: u64 = retained
            .iter()
            .chain(std::iter::once(&summary_message))
            .map(estimate_message_tokens)
            .sum();
        assert!(total <= 200, "compacted history used {total} tokens");
    }

    #[test]
    fn oversized_latest_message_is_summarized_instead_of_retained() {
        let messages = vec![user("before"), user(&"x".repeat(10_000))];
        let (retained, summaries) = compact_messages(&messages, 200).unwrap();
        assert!(retained.iter().all(|message| match message {
            Message::User(message) => message.content != "x".repeat(10_000),
            _ => true,
        }));
        assert!(summaries[0].contains("earlier messages summarized"));
    }

    #[test]
    fn unicode_truncation_does_not_panic() {
        assert_eq!(truncate_string("😀😀😀", 2), "..");
    }
}
