//! Print mode: single-shot prompt/response.
use std::io::{IsTerminal, Write};

use futures::StreamExt;
/// Sends a prompt to the LLM and prints the response, then exits.
use hallucinate_agent::{
    ai::types::{Context, Message, StreamOptions, UserMessage},
    config::settings::Settings,
};
use hallucinate_tui::components::{markdown_to_ansi, markdown_to_text};

use crate::cli::args::Cli;

/// Run print mode with the given CLI arguments and settings.
pub async fn run(
    cli: &Cli,
    settings: &Settings,
) -> Result<(), anyhow::Error> {
    let prompt = cli.print.as_deref().unwrap_or("");

    // Resolve model
    let model = hallucinate_agent::config::resolve_model(
        settings,
        cli.model.as_ref(),
        cli.base_url.as_ref(),
    )?;

    // Build context
    let images = futures::future::try_join_all(cli.images.iter().map(|path| {
        hallucinate_agent::ai::images::load_image_file(path, settings.auto_resize_images)
    }))
    .await?;

    let cwd = std::env::current_dir()?;

    let mut context = Context::new(vec![Message::User(UserMessage::with_images(
        prompt, images,
    ))]);

    let system_prompt = hallucinate_agent::agent::system_prompt::build_system_prompt_for_cwd(
        cli.system_prompt.as_deref(),
        Some(&cwd),
    );

    context = context.with_system_prompt(system_prompt);

    // Build stream options
    let mut options = StreamOptions::new();
    if let Some(api_key) = cli.api_key.as_deref().filter(|key| !key.is_empty()) {
        options = options.with_api_key(api_key);
    }

    if let Some(max_tokens) =
        hallucinate_agent::config::resolve_max_tokens(&settings, cli.max_tokens, &model)
    {
        options = options.with_max_tokens(max_tokens);
    }

    if let Some(temperature) = cli.temperature.or(settings.temperature) {
        options = options.with_temperature(temperature);
    }

    // Create provider for the model's provider group.
    let provider = hallucinate_agent::config::create_provider_for_model(
        settings,
        cli.base_url.as_ref(),
        cli.api_key.as_ref(),
        &model,
    )?;

    // Stream the response
    let mut stream = provider.stream(&model, &context, &options).await?;

    // Markdown needs the complete response in order to parse fenced code
    // blocks and other constructs correctly. Collect deltas and render once
    // the stream is complete instead of printing raw Markdown as it arrives.
    let show_thinking = hallucinate_agent::config::resolve_show_thinking(settings, cli.thinking);

    let mut response = String::new();
    let mut thinking = String::new();

    while let Some(event) = stream.next().await {
        match event {
            hallucinate_agent::ai::streaming::AssistantMessageEvent::TextDelta {
                delta, ..
            } => {
                response.push_str(&delta);
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::ThinkingDelta {
                delta,
                ..
            } => {
                thinking.push_str(&delta);
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::Done { message, .. } => {
                // Some providers include final content only in the Done
                // message, so use it as a fallback when no deltas arrived.
                if response.is_empty() {
                    response = message
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            hallucinate_agent::ai::types::AssistantContent::Text(text) => {
                                Some(text.text.as_str())
                            },
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                }

                if thinking.is_empty() {
                    thinking = message
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            hallucinate_agent::ai::types::AssistantContent::Thinking(thinking) => {
                                Some(thinking.thinking.as_str())
                            },
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }

                let output = visible_output(&response, &thinking, show_thinking);

                let rendered = if std::io::stdout().is_terminal() {
                    markdown_to_ansi(&output)
                } else {
                    markdown_to_text(&output)
                };

                print!("{rendered}");

                std::io::stdout().flush()?;
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::Error { error, .. } => {
                eprintln!(
                    "\n[Error: {}]",
                    error.error_message.as_deref().unwrap_or("unknown error")
                );

                return Err(anyhow::anyhow!(
                    "LLM error: {}",
                    error.error_message.as_deref().unwrap_or("unknown error")
                ));
            },

            hallucinate_agent::ai::streaming::AssistantMessageEvent::ToolcallEnd {
                tool_call,
                ..
            } => {
                println!("\n[Tool call: {}]", tool_call.human_readable());
            },
            _ => {},
        }
    }

    Ok(())
}

/// Add a clear, Markdown-friendly heading when reasoning output is enabled.
fn visible_output(
    response: &str,
    thinking: &str,
    show_thinking: bool,
) -> String {
    if !show_thinking || thinking.is_empty() {
        return response.to_string();
    }

    if response.is_empty() {
        format!("**Thinking**\n\n{thinking}")
    } else {
        format!("**Thinking**\n\n{thinking}\n\n{response}")
    }
}

#[cfg(test)]
mod tests {
    use super::visible_output;

    #[test]
    fn thinking_output_is_optional_and_precedes_the_answer() {
        assert_eq!(visible_output("answer", "plan", false), "answer");
        assert_eq!(
            visible_output("answer", "plan", true),
            "**Thinking**\n\nplan\n\nanswer"
        );
    }
}
