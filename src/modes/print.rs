//! Print mode: single-shot prompt/response.
use std::io::{IsTerminal, Write};

use futures::StreamExt;

/// Sends a prompt to the LLM and prints the response, then exits.
use crate::ai::types::{Context, Message, StreamOptions, UserMessage};
use crate::{
    cli::args::Cli,
    config::settings::Settings,
    tui::components::{markdown_to_ansi, markdown_to_text},
};

/// Run print mode with the given CLI arguments and settings.
pub async fn run(
    cli: &Cli,
    settings: &Settings,
) -> Result<(), anyhow::Error> {
    let prompt = cli.print.as_deref().unwrap_or("");

    // Resolve model
    let model = super::resolve_model(cli, settings)?;

    // Build context
    let images = cli
        .images
        .iter()
        .map(|path| crate::ai::images::load_image_file(path, settings.auto_resize_images))
        .collect::<Result<Vec<_>, _>>()?;
    let cwd = std::env::current_dir()?;
    let mut context = Context::new(vec![Message::User(UserMessage::with_images(
        prompt, images,
    ))]);
    let system_prompt = crate::agent::system_prompt::build_system_prompt_for_cwd(
        cli.system_prompt.as_deref(),
        Some(&cwd),
    );
    context = context.with_system_prompt(system_prompt);

    // Build stream options
    let mut options = StreamOptions::new();
    if let Some(api_key) = cli.api_key.as_deref().filter(|key| !key.is_empty()) {
        options = options.with_api_key(api_key);
    }
    if let Some(max_tokens) = super::resolve_max_tokens(cli, settings, &model) {
        options = options.with_max_tokens(max_tokens);
    }
    if let Some(temperature) = cli.temperature {
        options = options.with_temperature(temperature);
    }

    // Create provider for the model's provider group.
    let provider = super::create_provider_for_model(cli, settings, &model)?;

    // Stream the response
    let mut stream = provider.stream(&model, &context, &options).await?;

    // Markdown needs the complete response in order to parse fenced code
    // blocks and other constructs correctly. Collect deltas and render once
    // the stream is complete instead of printing raw Markdown as it arrives.
    let show_thinking = super::resolve_show_thinking(cli, settings);
    let mut response = String::new();
    let mut thinking = String::new();
    while let Some(event) = stream.next().await {
        match event {
            crate::ai::streaming::AssistantMessageEvent::TextDelta { delta, .. } => {
                response.push_str(&delta);
            },
            crate::ai::streaming::AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                thinking.push_str(&delta);
            },
            crate::ai::streaming::AssistantMessageEvent::Done { message, .. } => {
                // Some providers include final content only in the Done
                // message, so use it as a fallback when no deltas arrived.
                if response.is_empty() {
                    response = message
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            crate::ai::types::AssistantContent::Text(text) => {
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
                            crate::ai::types::AssistantContent::Thinking(thinking) => {
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
                println!(
                    "\n[Model: {} | Tokens: {} in, {} out]",
                    message.model, message.usage.input, message.usage.output
                );
            },
            crate::ai::streaming::AssistantMessageEvent::Error { error, .. } => {
                eprintln!(
                    "\n[Error: {}]",
                    error.error_message.as_deref().unwrap_or("unknown error")
                );
                return Err(anyhow::anyhow!(
                    "LLM error: {}",
                    error.error_message.as_deref().unwrap_or("unknown error")
                ));
            },
            crate::ai::streaming::AssistantMessageEvent::ToolcallEnd { tool_call, .. } => {
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
