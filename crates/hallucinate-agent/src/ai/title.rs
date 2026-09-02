//! LLM-assisted generation of concise session titles.

use futures::StreamExt;

use crate::ai::{
    provider::Provider,
    streaming::AssistantMessageEvent,
    types::{Context, Message, Model, StreamOptions, ToolChoice, UserMessage},
};

const TITLE_PROMPT: &str = "Generate a concise title for the user's request. Return only the title, with no quotation marks, markdown, or explanation. Use at most 80 characters.\n\nUser request:\n";

/// Generate a concise title for a session's first user message.
///
/// The request is deliberately text-only and tool-free so title generation
/// cannot affect the conversation or invoke workspace tools.
pub async fn generate_session_title(
    provider: &dyn Provider,
    model: &Model,
    user_message: &str,
) -> Result<String, anyhow::Error> {
    let context = Context::new([Message::User(UserMessage::new(format!(
        "{TITLE_PROMPT}{user_message}"
    )))]);
    let options = StreamOptions {
        max_tokens: Some(4096),
        tool_choice: Some(ToolChoice::None),
        ..StreamOptions::default()
    };
    let mut stream = provider.stream(model, &context, &options).await?;
    let mut title = String::new();

    while let Some(event) = stream.next().await {
        match event {
            AssistantMessageEvent::TextDelta { delta, .. } => title.push_str(&delta),
            AssistantMessageEvent::Error { error, .. } => {
                anyhow::bail!(
                    "title generation failed: {}",
                    error
                        .error_message
                        .unwrap_or_else(|| "unknown provider error".to_string())
                );
            },
            _ => {},
        }
    }

    normalize_title(&title).ok_or_else(|| anyhow::anyhow!("title generation returned no text"))
}

fn normalize_title(title: &str) -> Option<String> {
    let title = title
        .trim()
        .trim_matches(|character| matches!(character, '\'' | '"' | '`'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.chars().take(80).collect::<String>();

    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_a_model_title() {
        assert_eq!(
            normalize_title("  \"  Fix   title output  \"  "),
            Some("Fix title output".to_string())
        );
    }

    #[test]
    fn limits_title_length() {
        assert_eq!(
            normalize_title(&"a".repeat(81)).unwrap().chars().count(),
            80
        );
    }

    #[tokio::test]
    async fn requests_a_tool_free_title_from_the_provider() {
        use crate::ai::{
            providers::faux::{FauxProvider, FauxResponse},
            types::{Api, Model, StopReason},
        };

        let provider = FauxProvider::new();
        provider.set_responses(vec![FauxResponse::Text(
            "Fix title generation".to_string(),
            StopReason::Stop,
        )]);
        let model = Model::new(
            "faux-1",
            "Faux Model",
            Api::OpenaiCompletions,
            "faux",
            "http://localhost:0",
            false,
            128_000,
            16_384,
        );

        let title = generate_session_title(&provider, &model, "Please fix the title bug")
            .await
            .unwrap();

        assert_eq!(title, "Fix title generation");
        assert_eq!(
            provider.stream_options()[0].tool_choice,
            Some(ToolChoice::None)
        );
        assert!(provider.contexts()[0].tools.is_none());
    }
}
